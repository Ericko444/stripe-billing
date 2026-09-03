//! Task 6: `create_setup_intent` against wiremock + a real Postgres ledger.
//!
//! The idempotency-ledger state machine itself is covered generically in
//! `idempotency.rs`; this file asserts what is specific to
//! `create_setup_intent` -- the request it sends, that it carries and reuses
//! an idempotency key, that a Stripe 4xx is a typed error rather than a
//! panic, and that the ledger row completes with the SetupIntent's id.

mod common;

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use domain::{DomainError, TenantId};
use stripe_adapter::StripeBillingProvider;
use uuid::Uuid;
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, ResponseTemplate};

/// A minimal, valid `stripe_shared::SetupIntent` response body. The RC's
/// deserializer needs every non-nullable field present: `created`, `id`,
/// `livemode`, `payment_method_types`, `status` and `usage`. `client_secret`
/// is typed `Option` but a freshly created intent always has one, so the
/// fixture includes it -- it is the one field the adapter actually reads.
fn setup_intent_body(id: &str, client_secret: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "object": "setup_intent",
        "client_secret": client_secret,
        "created": 1_700_000_000,
        "livemode": false,
        "payment_method_types": ["card"],
        "status": "requires_payment_method",
        "usage": "off_session",
    })
}

/// A minimal, valid Stripe API error body -- `type` is required.
fn error_body(kind: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "error": { "type": kind, "message": message } })
}

fn key_of(req: &wiremock::Request) -> String {
    req.headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
async fn sends_the_customer_and_returns_the_client_secret() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());
    let pool = env.pool.clone();

    Mock::given(method("POST"))
        .and(path("/v1/setup_intents"))
        .respond_with(ResponseTemplate::new(200).set_body_json(setup_intent_body(
            "seti_created",
            "seti_created_secret_abc123",
        )))
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    let snapshot = provider.create_setup_intent(tenant, "cus_target").await?;

    assert_eq!(snapshot.client_secret, "seti_created_secret_abc123");

    let body = String::from_utf8(
        env.mock_server
            .received_requests()
            .await
            .ok_or("request recording must be enabled")?[0]
            .body
            .clone(),
    )?;
    assert!(
        body.contains("customer=cus_target"),
        "the customer id must be in the form body: {body}"
    );
    // D4: the Element confirms inline with no `return_url`, which only holds
    // while the intent is scoped to cards. Unset, this account's automatic
    // payment methods put a redirect-based wallet first. Asserted here
    // because the scoping is load-bearing for the frontend, not cosmetic --
    // and because it is also a fingerprint input (see `PAYMENT_METHOD_TYPES`).
    assert!(
        body.contains("payment_method_types[0]=card"),
        "the intent must be scoped to card: {body}"
    );

    // The ledger row is complete and carries the SetupIntent's id.
    let (object_id, completed): (Option<String>, bool) = sqlx::query_as(
        "SELECT stripe_object_id, completed_at IS NOT NULL \
         FROM billing.outbound_requests WHERE tenant_id = $1",
    )
    .bind(tenant.as_uuid())
    .fetch_one(&pool)
    .await?;
    assert!(completed, "the reservation must be marked complete");
    assert_eq!(object_id.as_deref(), Some("seti_created"));

    Ok(())
}

#[tokio::test]
async fn carries_an_idempotency_key_header() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    // `header_exists` is a matcher: if the header is absent the mock never
    // matches, the request 404s, and `expect(1)` fails on drop.
    Mock::given(method("POST"))
        .and(path("/v1/setup_intents"))
        .and(header_exists("idempotency-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(setup_intent_body("seti_hdr", "seti_hdr_secret_x")),
        )
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    provider.create_setup_intent(tenant, "cus_hdr").await?;

    Ok(())
}

#[tokio::test]
async fn a_retry_of_the_same_operation_reuses_the_key() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    // First attempt fails (simulated outage); the retry, same inputs,
    // succeeds. `RequestStrategy::Idempotent` makes exactly one HTTP attempt
    // per `send()`, so each call below is one responder invocation.
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = calls.clone();
    Mock::given(method("POST"))
        .and(path("/v1/setup_intents"))
        .respond_with(move |_req: &wiremock::Request| {
            let attempt = responder_calls.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                ResponseTemplate::new(500)
                    .set_body_json(error_body("api_error", "simulated transient failure"))
            } else {
                ResponseTemplate::new(200)
                    .set_body_json(setup_intent_body("seti_retry", "seti_retry_secret_y"))
            }
        })
        .expect(2)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;

    let first = provider.create_setup_intent(tenant, "cus_retry").await;
    assert!(first.is_err(), "the simulated 500 must surface as an error");
    let second = provider.create_setup_intent(tenant, "cus_retry").await;
    assert!(second.is_ok(), "the retry must succeed");

    let requests = env
        .mock_server
        .received_requests()
        .await
        .ok_or("request recording must be enabled")?;
    assert_eq!(requests.len(), 2, "exactly two attempts reached Stripe");
    let keys: Vec<String> = requests.iter().map(key_of).collect();
    assert!(!keys[0].is_empty(), "the first attempt must carry a key");
    assert_eq!(
        keys[0], keys[1],
        "the retry of the same logical operation must reuse the exact key"
    );

    Ok(())
}

#[tokio::test]
async fn a_4xx_from_stripe_is_a_provider_error_not_a_panic() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    Mock::given(method("POST"))
        .and(path("/v1/setup_intents"))
        .respond_with(
            ResponseTemplate::new(402)
                .set_body_json(error_body("card_error", "your card was declined")),
        )
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    match provider.create_setup_intent(tenant, "cus_4xx").await {
        Err(DomainError::Provider(_)) => Ok(()),
        other => Err(format!("expected DomainError::Provider, got {other:?}").into()),
    }
}
