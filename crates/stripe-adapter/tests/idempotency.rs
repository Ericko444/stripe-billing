mod common;

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use domain::{CreateCustomerParams, DomainError, OutboundRequestId, TenantId};
use stripe_adapter::{Ledger, StripeBillingProvider};
use uuid::Uuid;
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, ResponseTemplate};

/// A minimal, valid `stripe_shared::Customer` response body: `id`,
/// `object`, `created` and `livemode` are the type's only non-`Option`
/// fields.
fn customer_body(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "object": "customer",
        "created": 1_700_000_000,
        "livemode": false,
    })
}

/// A minimal, valid Stripe API error body. `type` is required --
/// `stripe_shared::ApiErrors::type_` is not `Option`.
fn error_body(kind: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "error": { "type": kind, "message": message } })
}

// --- The named B6 deliverable -----------------------------------------

#[tokio::test]
async fn retry_after_failure_reuses_the_same_idempotency_key() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());
    let params = CreateCustomerParams {
        email: Some("b6@example.com".to_string()),
        name: None,
    };

    // First attempt fails (simulated transient outage); the retry, with
    // identical inputs, succeeds. RequestStrategy::Idempotent makes exactly
    // one HTTP attempt per send() -- confirmed against the SDK's own retry
    // decision loop (request_strategy.rs's `test()`, called from
    // hyper/client.rs's send_inner) -- so each create_customer() call below
    // corresponds to exactly one responder invocation.
    let call_count = Arc::new(AtomicUsize::new(0));
    let responder_calls = call_count.clone();
    Mock::given(method("POST"))
        .and(path("/v1/customers"))
        .respond_with(move |_req: &wiremock::Request| {
            let attempt = responder_calls.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                ResponseTemplate::new(500)
                    .set_body_json(error_body("api_error", "simulated transient failure"))
            } else {
                ResponseTemplate::new(200).set_body_json(customer_body("cus_b6"))
            }
        })
        .expect(2)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;

    let first = provider.create_customer(tenant, params.clone()).await;
    assert!(
        first.is_err(),
        "the first, simulated-failure attempt must return an error"
    );

    let second = provider.create_customer(tenant, params).await;
    assert!(
        second.is_ok(),
        "the retry must succeed once Stripe accepts it"
    );

    let requests = env
        .mock_server
        .received_requests()
        .await
        .ok_or("request recording must be enabled")?;
    assert_eq!(
        requests.len(),
        2,
        "exactly two attempts must have reached Stripe"
    );

    let keys: Vec<&str> = requests
        .iter()
        .map(|r| {
            r.headers
                .get("idempotency-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
        })
        .collect();
    assert!(!keys[0].is_empty(), "the first attempt must carry a key");
    assert_eq!(
        keys[0], keys[1],
        "the retry must reuse the exact same idempotency key as the failed attempt"
    );

    Ok(())
}

// --- Every mutating call carries the header at all ---------------------

#[tokio::test]
async fn every_mutating_call_carries_an_idempotency_key_header() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());
    let params = CreateCustomerParams {
        email: Some("header-check@example.com".to_string()),
        name: None,
    };

    // header_exists is a matcher, not an assertion after the fact: if the
    // header is absent the mock never matches, the request fails, and
    // verify() below reports the unmet expectation.
    Mock::given(method("POST"))
        .and(path("/v1/customers"))
        .and(header_exists("idempotency-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(customer_body("cus_header_check")))
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    provider.create_customer(tenant, params).await?;

    env.mock_server.verify().await;
    Ok(())
}

// --- Case C: a completed row within the window reuses its key ----------

#[tokio::test]
async fn completed_row_within_the_window_reuses_the_key_case_c() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());
    let params = CreateCustomerParams {
        email: Some("case-c@example.com".to_string()),
        name: None,
    };

    Mock::given(method("POST"))
        .and(path("/v1/customers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(customer_body("cus_case_c")))
        .expect(2)
        .mount(&env.mock_server)
        .await;

    let pool = env.pool.clone();
    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;

    provider.create_customer(tenant, params.clone()).await?;
    provider.create_customer(tenant, params).await?;

    let requests = env
        .mock_server
        .received_requests()
        .await
        .ok_or("request recording must be enabled")?;
    assert_eq!(
        requests.len(),
        2,
        "Stripe must be called again on reuse -- its own dedup, not ours, \
         is what makes the second call safe"
    );

    let keys: Vec<&str> = requests
        .iter()
        .map(|r| {
            r.headers
                .get("idempotency-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
        })
        .collect();
    assert!(!keys[0].is_empty());
    assert_eq!(keys[0], keys[1], "the reused key must be identical");

    let row_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM billing.outbound_requests WHERE tenant_id = $1")
            .bind(tenant.as_uuid())
            .fetch_one(&pool)
            .await?;
    assert_eq!(row_count, 1, "reuse must not create a second ledger row");

    Ok(())
}

// --- Case D: a completed row past the window gets a fresh key ----------

#[tokio::test]
async fn completed_row_past_the_window_gets_a_fresh_key_case_d() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());
    let params = CreateCustomerParams {
        email: Some("case-d@example.com".to_string()),
        name: None,
    };

    Mock::given(method("POST"))
        .and(path("/v1/customers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(customer_body("cus_case_d")))
        .expect(2)
        .mount(&env.mock_server)
        .await;

    let pool = env.pool.clone();
    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;

    provider.create_customer(tenant, params.clone()).await?;

    let row_id: Uuid =
        sqlx::query_scalar("SELECT id FROM billing.outbound_requests WHERE tenant_id = $1")
            .bind(tenant.as_uuid())
            .fetch_one(&pool)
            .await?;
    common::backdate(&pool, OutboundRequestId::new(row_id), 30).await?;

    provider.create_customer(tenant, params).await?;

    let requests = env
        .mock_server
        .received_requests()
        .await
        .ok_or("request recording must be enabled")?;
    assert_eq!(requests.len(), 2);

    let keys: Vec<&str> = requests
        .iter()
        .map(|r| {
            r.headers
                .get("idempotency-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
        })
        .collect();
    assert_ne!(
        keys[0], keys[1],
        "a fingerprint recurring past the key window is a genuinely new \
         operation and must not reuse the old key"
    );

    let row_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM billing.outbound_requests WHERE tenant_id = $1")
            .bind(tenant.as_uuid())
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        row_count, 2,
        "a genuinely new operation must insert a second row"
    );

    Ok(())
}

// --- Case E: an incomplete row past the window is a typed error --------

#[tokio::test]
async fn incomplete_row_past_the_window_errors_without_calling_stripe_case_e()
-> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());
    let params = CreateCustomerParams {
        email: Some("case-e@example.com".to_string()),
        name: None,
    };

    // Only ever mounted once: the second create_customer call below must
    // never reach this mock at all.
    Mock::given(method("POST"))
        .and(path("/v1/customers"))
        .respond_with(ResponseTemplate::new(500).set_body_json(error_body(
            "api_error",
            "simulated outage, leaving the row incomplete",
        )))
        .up_to_n_times(1)
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let pool = env.pool.clone();
    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;

    let first = provider.create_customer(tenant, params.clone()).await;
    assert!(first.is_err(), "the simulated 500 must surface as an error");

    let row_id: Uuid =
        sqlx::query_scalar("SELECT id FROM billing.outbound_requests WHERE tenant_id = $1")
            .bind(tenant.as_uuid())
            .fetch_one(&pool)
            .await?;
    common::backdate(&pool, OutboundRequestId::new(row_id), 30).await?;

    let requests_before = env
        .mock_server
        .received_requests()
        .await
        .ok_or("request recording must be enabled")?
        .len();

    let second = provider.create_customer(tenant, params).await;
    match second {
        Err(DomainError::Provider(_)) => {}
        other => return Err(format!("expected DomainError::Provider, got {other:?}").into()),
    }

    let requests_after = env
        .mock_server
        .received_requests()
        .await
        .ok_or("request recording must be enabled")?
        .len();
    assert_eq!(
        requests_before, requests_after,
        "case E must not place a second call to Stripe -- reusing the key no \
         longer dedups, and a fresh key risks a duplicate if the original \
         attempt actually landed"
    );

    Ok(())
}

// --- Case F: a concurrent race mints exactly one key --------------------

#[tokio::test]
async fn concurrent_reserves_for_one_fingerprint_mint_exactly_one_key_case_f()
-> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());
    let pool = env.pool.clone();
    let ledger = Ledger::new(env.repo);

    // Asserting on the resulting row count, not on which future "won" --
    // that ordering is not a stable thing to assert on, but "only one key
    // exists afterward" is the actual invariant this case protects.
    let (first, second) = tokio::join!(
        ledger.reserve(tenant, "create_customer", "fp_case_f"),
        ledger.reserve(tenant, "create_customer", "fp_case_f"),
    );
    let first = first?;
    let second = second?;
    assert_eq!(
        first.idempotency_key, second.idempotency_key,
        "the loser of the race must reuse the winner's key, not mint its own"
    );

    let row_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM billing.outbound_requests \
         WHERE tenant_id = $1 AND operation = $2 AND request_fingerprint = $3",
    )
    .bind(tenant.as_uuid())
    .bind("create_customer")
    .bind("fp_case_f")
    .fetch_one(&pool)
    .await?;
    assert_eq!(row_count, 1, "exactly one row must exist despite the race");

    Ok(())
}

// --- A 4xx from Stripe leaves the ledger row incomplete ------------------

#[tokio::test]
async fn a_4xx_from_stripe_returns_a_provider_error_and_leaves_the_row_incomplete()
-> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());
    let params = CreateCustomerParams {
        email: Some("declined@example.com".to_string()),
        name: None,
    };

    Mock::given(method("POST"))
        .and(path("/v1/customers"))
        .respond_with(
            ResponseTemplate::new(402)
                .set_body_json(error_body("card_error", "Your card was declined.")),
        )
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let pool = env.pool.clone();
    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;

    let result = provider.create_customer(tenant, params).await;
    let message = match result {
        Err(DomainError::Provider(message)) => message,
        other => return Err(format!("expected DomainError::Provider, got {other:?}").into()),
    };
    assert!(
        message.contains("Your card was declined."),
        "the domain-facing message should surface Stripe's own error text: {message}"
    );

    let completed_at: Option<time::OffsetDateTime> = sqlx::query_scalar(
        "SELECT completed_at FROM billing.outbound_requests WHERE tenant_id = $1",
    )
    .bind(tenant.as_uuid())
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        completed_at, None,
        "a 4xx must leave the ledger row incomplete"
    );

    Ok(())
}
