//! Router-level tests for `GET /payment-methods`, through `billing_router<T>`.
//! Assertions: tenant isolation, soft-deleted rows stay hidden, and the DTO
//! never widens past display metadata.

mod common;

use std::collections::BTreeSet;
use std::error::Error;

use api::billing_router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use common::{HeaderTenant, StubReads, app_state};
use domain::{CustomerId, PaymentMethod, PaymentMethodId, TenantId};
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

fn payment_method(tenant: TenantId, last4: &str, is_default: bool) -> PaymentMethod {
    PaymentMethod {
        id: PaymentMethodId::new(Uuid::new_v4()),
        tenant_id: tenant,
        customer_id: CustomerId::new(Uuid::new_v4()),
        stripe_payment_method_id: format!("pm_{}", Uuid::new_v4()),
        brand: "visa".to_string(),
        last4: last4.to_string(),
        is_default,
        last_event_created_at: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
        deleted_at: None,
    }
}

async fn get_payment_methods(
    state: api::AppState,
    tenant: &str,
) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let response = billing_router::<HeaderTenant>(state)
        .oneshot(
            Request::get("/payment-methods")
                .header("x-tenant", tenant)
                .body(Body::empty())?,
        )
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Ok((status, body))
}

#[tokio::test]
async fn returns_only_the_calling_tenants_cards() -> Result<(), Box<dyn Error>> {
    let mine = TenantId::new(Uuid::new_v4());
    let theirs = TenantId::new(Uuid::new_v4());
    let state = app_state(StubReads {
        payment_methods: vec![
            payment_method(mine, "4242", true),
            payment_method(mine, "1881", false),
            payment_method(theirs, "9999", true),
        ],
        ..Default::default()
    });

    let (status, body) = get_payment_methods(state, &mine.as_uuid().to_string()).await?;

    assert_eq!(status, StatusCode::OK);
    let last4s: Vec<&str> = body
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pm| pm["last4"].as_str())
        .collect();
    assert_eq!(last4s, ["4242", "1881"]);
    assert!(!body.to_string().contains("9999"));
    Ok(())
}

#[tokio::test]
async fn soft_deleted_cards_are_excluded() -> Result<(), Box<dyn Error>> {
    let tenant = TenantId::new(Uuid::new_v4());
    let mut deleted = payment_method(tenant, "0000", false);
    deleted.deleted_at = Some(OffsetDateTime::UNIX_EPOCH);
    let state = app_state(StubReads {
        payment_methods: vec![payment_method(tenant, "4242", true), deleted],
        ..Default::default()
    });

    let (status, body) = get_payment_methods(state, &tenant.as_uuid().to_string()).await?;

    assert_eq!(status, StatusCode::OK);
    let last4s: Vec<&str> = body
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pm| pm["last4"].as_str())
        .collect();
    assert_eq!(last4s, ["4242"]);
    Ok(())
}

#[tokio::test]
async fn the_dto_carries_no_field_beyond_display_metadata() -> Result<(), Box<dyn Error>> {
    let tenant = TenantId::new(Uuid::new_v4());
    let state = app_state(StubReads {
        payment_methods: vec![payment_method(tenant, "4242", true)],
        ..Default::default()
    });

    let (status, body) = get_payment_methods(state, &tenant.as_uuid().to_string()).await?;

    assert_eq!(status, StatusCode::OK);
    let keys: BTreeSet<&str> = body[0]
        .as_object()
        .into_iter()
        .flat_map(|obj| obj.keys().map(String::as_str))
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["id", "brand", "last4", "is_default", "created_at"]),
    );
    // Nothing that would betray card data or the Stripe token.
    let rendered = body.to_string();
    assert!(!rendered.contains("pm_"));
    assert!(!rendered.contains("customer_id"));
    assert!(!rendered.contains("stripe"));
    Ok(())
}
