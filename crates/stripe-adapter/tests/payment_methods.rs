//! Task 10: `set_default_payment_method` and `detach_payment_method` against
//! wiremock + a real Postgres ledger.
//!
//! The idempotency-ledger state machine is covered generically in
//! `idempotency.rs`; here we assert what is specific to each call -- the
//! request shape it sends, that it carries and reuses an idempotency key,
//! that `detach`'s key survives a retry (its fingerprint has no timestamp),
//! and that a Stripe error is a typed `DomainError`, never a panic.

mod common;

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use domain::{DomainError, TenantId};
use stripe_adapter::StripeBillingProvider;
use uuid::Uuid;
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, ResponseTemplate};

/// A minimal, valid `stripe_shared::Customer` body: `id`, `object`,
/// `created` and `livemode` are the type's only non-`Option` fields.
fn customer_body(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "object": "customer",
        "created": 1_700_000_000,
        "livemode": false,
    })
}

/// A minimal, valid `stripe_shared::PaymentMethod` body. Non-`Option` fields
/// are `id`, `object`, `billing_details`, `created`, `livemode` and `type`;
/// `billing_details` is itself all-`Option`, so `{}` satisfies it.
fn payment_method_body(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "object": "payment_method",
        "billing_details": {},
        "created": 1_700_000_000,
        "livemode": false,
        "type": "card",
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

// --- set_default_payment_method ---------------------------------------

#[tokio::test]
async fn set_default_updates_invoice_settings_on_the_customer() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());
    let pool = env.pool.clone();

    Mock::given(method("POST"))
        .and(path("/v1/customers/cus_target"))
        .and(header_exists("idempotency-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(customer_body("cus_target")))
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    provider
        .set_default_payment_method(tenant, "cus_target", "pm_new_default")
        .await?;

    let body = String::from_utf8(
        env.mock_server
            .received_requests()
            .await
            .ok_or("request recording must be enabled")?[0]
            .body
            .clone(),
    )?;
    assert!(
        body.contains("invoice_settings[default_payment_method]=pm_new_default")
            || body.contains("invoice_settings%5Bdefault_payment_method%5D=pm_new_default"),
        "the default payment method must be sent under invoice_settings: {body}"
    );

    let (object_id, completed): (Option<String>, bool) = sqlx::query_as(
        "SELECT stripe_object_id, completed_at IS NOT NULL \
         FROM billing.outbound_requests WHERE tenant_id = $1",
    )
    .bind(tenant.as_uuid())
    .fetch_one(&pool)
    .await?;
    assert!(completed, "the reservation must be marked complete");
    assert_eq!(object_id.as_deref(), Some("cus_target"));

    Ok(())
}

#[tokio::test]
async fn set_default_reuses_the_key_on_retry() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = calls.clone();
    Mock::given(method("POST"))
        .and(path("/v1/customers/cus_retry"))
        .respond_with(move |_req: &wiremock::Request| {
            let attempt = responder_calls.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                ResponseTemplate::new(500)
                    .set_body_json(error_body("api_error", "simulated transient failure"))
            } else {
                ResponseTemplate::new(200).set_body_json(customer_body("cus_retry"))
            }
        })
        .expect(2)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    assert!(
        provider
            .set_default_payment_method(tenant, "cus_retry", "pm_x")
            .await
            .is_err()
    );
    provider
        .set_default_payment_method(tenant, "cus_retry", "pm_x")
        .await?;

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

// --- detach_payment_method -------------------------------------------

#[tokio::test]
async fn detach_posts_to_the_detach_endpoint() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());
    let pool = env.pool.clone();

    Mock::given(method("POST"))
        .and(path("/v1/payment_methods/pm_gone/detach"))
        .and(header_exists("idempotency-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(payment_method_body("pm_gone")))
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    provider.detach_payment_method(tenant, "pm_gone").await?;

    let (object_id, completed): (Option<String>, bool) = sqlx::query_as(
        "SELECT stripe_object_id, completed_at IS NOT NULL \
         FROM billing.outbound_requests WHERE tenant_id = $1",
    )
    .bind(tenant.as_uuid())
    .fetch_one(&pool)
    .await?;
    assert!(completed, "the reservation must be marked complete");
    assert_eq!(object_id.as_deref(), Some("pm_gone"));

    Ok(())
}

#[tokio::test]
async fn detach_retry_reuses_the_key_and_does_not_re_detach() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    // The fingerprint is tenant + payment method id only. A second detach of
    // the same card, moments later, must reuse the first key -- otherwise a
    // retry could detach a card the customer re-attached in between.
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = calls.clone();
    Mock::given(method("POST"))
        .and(path("/v1/payment_methods/pm_twice/detach"))
        .respond_with(move |_req: &wiremock::Request| {
            let attempt = responder_calls.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                ResponseTemplate::new(500)
                    .set_body_json(error_body("api_error", "simulated transient failure"))
            } else {
                ResponseTemplate::new(200).set_body_json(payment_method_body("pm_twice"))
            }
        })
        .expect(2)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    assert!(
        provider
            .detach_payment_method(tenant, "pm_twice")
            .await
            .is_err()
    );
    provider.detach_payment_method(tenant, "pm_twice").await?;

    let requests = env
        .mock_server
        .received_requests()
        .await
        .ok_or("request recording must be enabled")?;
    let keys: Vec<String> = requests.iter().map(key_of).collect();
    assert_eq!(keys.len(), 2);
    assert!(!keys[0].is_empty());
    assert_eq!(
        keys[0], keys[1],
        "detach's key must survive a retry -- its fingerprint carries no timestamp"
    );

    Ok(())
}

#[tokio::test]
async fn detach_4xx_is_a_provider_error_not_a_panic() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    Mock::given(method("POST"))
        .and(path("/v1/payment_methods/pm_bad/detach"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(error_body("invalid_request_error", "No such PaymentMethod")),
        )
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    match provider.detach_payment_method(tenant, "pm_bad").await {
        Err(DomainError::Provider(_)) => Ok(()),
        other => Err(format!("expected DomainError::Provider, got {other:?}").into()),
    }
}
