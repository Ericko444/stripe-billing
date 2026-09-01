//! Router-level tests for `GET /subscription`, through `billing_router<T>`.
//! The load-bearing assertions: a tenant with no subscription gets 200 (not
//! 404), and no response carries another tenant's subscription.

mod common;

use std::error::Error;

use api::billing_router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use common::{HeaderTenant, StubReads, app_state};
use domain::{CustomerId, PlanId, Subscription, SubscriptionId, SubscriptionStatus, TenantId};
use serde_json::Value;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

fn subscription(tenant: TenantId, plan_id: PlanId, status: SubscriptionStatus) -> Subscription {
    let start = OffsetDateTime::UNIX_EPOCH + Duration::days(100);
    Subscription {
        id: SubscriptionId::new(Uuid::new_v4()),
        tenant_id: tenant,
        customer_id: CustomerId::new(Uuid::new_v4()),
        plan_id,
        stripe_subscription_id: format!("sub_{}", Uuid::new_v4()),
        stripe_subscription_item_id: "si_test".to_string(),
        status,
        current_period_start: start,
        current_period_end: start + Duration::days(30),
        cancel_at_period_end: false,
        last_event_created_at: None,
        created_at: start,
        deleted_at: None,
    }
}

async fn get_subscription(
    state: api::AppState,
    tenant: &str,
) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let response = billing_router::<HeaderTenant>(state)
        .oneshot(
            Request::get("/subscription")
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
async fn returns_the_tenants_subscription_with_status_and_period_bounds()
-> Result<(), Box<dyn Error>> {
    let tenant = TenantId::new(Uuid::new_v4());
    let plan_id = PlanId::new(Uuid::new_v4());
    let sub = subscription(tenant, plan_id, SubscriptionStatus::Active);
    let sub_id = sub.id;
    let state = app_state(StubReads {
        subscriptions: vec![sub],
        ..Default::default()
    });

    let (status, body) = get_subscription(state, &tenant.as_uuid().to_string()).await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], sub_id.as_uuid().to_string());
    assert_eq!(body["status"], "active");
    // Local plan id, never the Stripe price id.
    assert_eq!(body["plan_id"], plan_id.as_uuid().to_string());
    assert!(!body.to_string().contains("price_"));
    // Both period bounds are present, RFC 3339 UTC. `subscription()` starts
    // the period 100 days after the epoch and runs it 30 days.
    assert_eq!(body["current_period_start"], "1970-04-11T00:00:00Z");
    assert_eq!(body["current_period_end"], "1970-05-11T00:00:00Z");
    assert_eq!(body["cancel_at_period_end"], false);
    Ok(())
}

#[tokio::test]
async fn a_tenant_with_no_subscription_gets_200_and_null() -> Result<(), Box<dyn Error>> {
    let tenant = TenantId::new(Uuid::new_v4());
    let state = app_state(StubReads::default());

    let (status, body) = get_subscription(state, &tenant.as_uuid().to_string()).await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, Value::Null);
    Ok(())
}

#[tokio::test]
async fn never_returns_another_tenants_subscription() -> Result<(), Box<dyn Error>> {
    let mine = TenantId::new(Uuid::new_v4());
    let theirs = TenantId::new(Uuid::new_v4());
    let their_plan = PlanId::new(Uuid::new_v4());
    let their_sub = subscription(theirs, their_plan, SubscriptionStatus::Active);
    let their_sub_id = their_sub.id;
    let state = app_state(StubReads {
        subscriptions: vec![their_sub],
        ..Default::default()
    });

    // Tenant B does see theirs...
    let (status_b, body_b) = get_subscription(state.clone(), &theirs.as_uuid().to_string()).await?;
    assert_eq!(status_b, StatusCode::OK);
    assert_eq!(body_b["id"], their_sub_id.as_uuid().to_string());

    // ...tenant A sees nothing, and none of B's ids leak.
    let (status_a, body_a) = get_subscription(state, &mine.as_uuid().to_string()).await?;
    assert_eq!(status_a, StatusCode::OK);
    assert_eq!(body_a, Value::Null);
    assert!(
        !body_a
            .to_string()
            .contains(&their_sub_id.as_uuid().to_string())
    );
    assert!(
        !body_a
            .to_string()
            .contains(&their_plan.as_uuid().to_string())
    );
    Ok(())
}
