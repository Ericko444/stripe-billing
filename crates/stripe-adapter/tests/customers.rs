//! `update_customer` against wiremock + a real Postgres ledger.
//!
//! The idempotency-ledger behaviour itself is exercised once, generically,
//! in `idempotency.rs`; this file covers what is specific to
//! `update_customer` -- that it sends a partial body, and that its
//! fingerprint never collides with a create for the same customer.

mod common;

use std::error::Error;

use domain::{CreateCustomerParams, TenantId, UpdateCustomerParams};
use stripe_adapter::StripeBillingProvider;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// A minimal, valid `stripe_shared::Customer` response body -- `id`,
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

#[tokio::test]
async fn update_customer_sends_only_the_fields_that_are_present() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    Mock::given(method("POST"))
        .and(path("/v1/customers/cus_partial"))
        .respond_with(ResponseTemplate::new(200).set_body_json(customer_body("cus_partial")))
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    provider
        .update_customer(
            tenant,
            "cus_partial",
            UpdateCustomerParams {
                email: Some("new@example.com".to_string()),
                name: None,
            },
        )
        .await?;

    let requests = env
        .mock_server
        .received_requests()
        .await
        .ok_or("request recording must be enabled")?;
    let body = String::from_utf8(requests[0].body.clone())?;
    assert!(
        body.contains("email=new%40example.com") || body.contains("email=new@example.com"),
        "the present field must be in the form body: {body}"
    );
    assert!(
        !body.contains("name="),
        "an absent field must not be sent at all -- Stripe treats a present \
         empty value as 'clear this attribute': {body}"
    );

    Ok(())
}

#[tokio::test]
async fn an_update_never_shares_a_key_with_a_create_for_the_same_customer()
-> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());
    let params_email = Some("same@example.com".to_string());

    Mock::given(method("POST"))
        .and(path("/v1/customers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(customer_body("cus_same")))
        .expect(1)
        .mount(&env.mock_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/customers/cus_same"))
        .respond_with(ResponseTemplate::new(200).set_body_json(customer_body("cus_same")))
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let pool = env.pool.clone();
    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;

    provider
        .create_customer(
            tenant,
            CreateCustomerParams {
                email: params_email.clone(),
                name: None,
            },
        )
        .await?;
    provider
        .update_customer(
            tenant,
            "cus_same",
            UpdateCustomerParams {
                email: params_email,
                name: None,
            },
        )
        .await?;

    let requests = env
        .mock_server
        .received_requests()
        .await
        .ok_or("request recording must be enabled")?;
    let keys: Vec<&str> = requests
        .iter()
        .map(|r| {
            r.headers
                .get("idempotency-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
        })
        .collect();
    assert_eq!(keys.len(), 2, "one create call and one update call");
    assert!(!keys[0].is_empty() && !keys[1].is_empty());
    assert_ne!(
        keys[0], keys[1],
        "create and update must fingerprint differently even with identical \
         inputs -- otherwise the update would reuse the create's key and \
         Stripe would replay the create"
    );

    let row_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM billing.outbound_requests WHERE tenant_id = $1")
            .bind(tenant.as_uuid())
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        row_count, 2,
        "the create and the update are separate ledger rows"
    );

    Ok(())
}
