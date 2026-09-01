//! Task 14: `create_checkout_session` against wiremock + a real Postgres
//! ledger.
//!
//! The idempotency-ledger state machine is covered generically in
//! `idempotency.rs`; here we assert the request shape (`mode=subscription`,
//! the customer, the price, both URLs), that the key survives a retry, and
//! that the snapshot carries the session `url` and id and nothing else.

mod common;

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use domain::{CheckoutSessionParams, TenantId};
use stripe_adapter::StripeBillingProvider;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// A minimal, valid `stripe_shared::CheckoutSession` body. Non-`Option`
/// fields are `id`, `object`, `automatic_tax`, `created`, `custom_fields`,
/// `custom_text`, `expires_at`, `livemode`, `mode`, `payment_method_types`,
/// `payment_status` and `shipping_options`; the two nested objects are
/// themselves all-`Option` bar `automatic_tax.enabled`. `url` is typed
/// `Option` but a `subscription`-mode session always has one -- it is the
/// field the adapter reads.
fn checkout_session_body(id: &str, url: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "object": "checkout.session",
        "automatic_tax": { "enabled": false },
        "created": 1_700_000_000,
        "custom_fields": [],
        "custom_text": {},
        "expires_at": 1_700_086_400,
        "livemode": false,
        "mode": "subscription",
        "payment_method_types": ["card"],
        "payment_status": "unpaid",
        "shipping_options": [],
        "url": url,
    })
}

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

fn params() -> CheckoutSessionParams {
    CheckoutSessionParams {
        stripe_customer_id: "cus_checkout".to_string(),
        stripe_price_id: "price_checkout".to_string(),
        success_url: "https://app.example.com/billing/done".to_string(),
        cancel_url: "https://app.example.com/billing".to_string(),
    }
}

#[tokio::test]
async fn sends_subscription_mode_and_returns_the_url() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());
    let pool = env.pool.clone();

    Mock::given(method("POST"))
        .and(path("/v1/checkout/sessions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(checkout_session_body(
                "cs_test_created",
                "https://checkout.stripe.com/c/pay/cs_test_created",
            )),
        )
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    let snapshot = provider.create_checkout_session(tenant, params()).await?;

    assert_eq!(
        snapshot.url,
        "https://checkout.stripe.com/c/pay/cs_test_created"
    );
    assert_eq!(snapshot.stripe_session_id, "cs_test_created");

    let body = String::from_utf8(
        env.mock_server
            .received_requests()
            .await
            .ok_or("request recording must be enabled")?[0]
            .body
            .clone(),
    )?;
    assert!(body.contains("mode=subscription"), "mode: {body}");
    assert!(body.contains("customer=cus_checkout"), "customer: {body}");
    assert!(
        body.contains("line_items[0][price]=price_checkout"),
        "price: {body}"
    );
    assert!(body.contains("success_url="), "success_url: {body}");
    assert!(body.contains("cancel_url="), "cancel_url: {body}");

    let (object_id, completed): (Option<String>, bool) = sqlx::query_as(
        "SELECT stripe_object_id, completed_at IS NOT NULL \
         FROM billing.outbound_requests WHERE tenant_id = $1",
    )
    .bind(tenant.as_uuid())
    .fetch_one(&pool)
    .await?;
    assert!(completed, "the reservation must be marked complete");
    assert_eq!(object_id.as_deref(), Some("cs_test_created"));

    Ok(())
}

#[tokio::test]
async fn reuses_the_key_on_retry() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = calls.clone();
    Mock::given(method("POST"))
        .and(path("/v1/checkout/sessions"))
        .respond_with(move |_req: &wiremock::Request| {
            let attempt = responder_calls.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                ResponseTemplate::new(500)
                    .set_body_json(error_body("api_error", "simulated transient failure"))
            } else {
                ResponseTemplate::new(200).set_body_json(checkout_session_body(
                    "cs_test_retry",
                    "https://checkout.stripe.com/c/pay/cs_test_retry",
                ))
            }
        })
        .expect(2)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    assert!(
        provider
            .create_checkout_session(tenant, params())
            .await
            .is_err()
    );
    provider.create_checkout_session(tenant, params()).await?;

    let requests = env
        .mock_server
        .received_requests()
        .await
        .ok_or("request recording must be enabled")?;
    let keys: Vec<String> = requests.iter().map(key_of).collect();
    assert_eq!(keys.len(), 2);
    assert!(!keys[0].is_empty());
    assert_eq!(keys[0], keys[1], "the retry must reuse the exact key");

    Ok(())
}
