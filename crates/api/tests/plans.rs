//! Router-level tests for `GET /plans`, driven through `billing_router<T>`
//! with a stub tenant extractor. The load-bearing assertion is tenant
//! isolation: each tenant's `GET /plans` returns only its own rows.

mod common;

use std::error::Error;

use api::billing_router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use common::{HeaderTenant, StubReads, app_state};
use domain::{Currency, Money, Plan, PlanId, TenantId};
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

fn plan(tenant: TenantId, name: &str, amount_minor: i64) -> Plan {
    Plan {
        id: PlanId::new(Uuid::new_v4()),
        tenant_id: tenant,
        stripe_price_id: format!("price_{name}"),
        stripe_product_id: format!("prod_{name}"),
        name: name.to_string(),
        amount: Money::new(amount_minor, Currency::Eur),
        created_at: OffsetDateTime::UNIX_EPOCH,
        deleted_at: None,
    }
}

async fn get_plans(
    state: api::AppState,
    tenant: &str,
) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let response = billing_router::<HeaderTenant>(state)
        .oneshot(
            Request::get("/plans")
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
async fn returns_only_the_calling_tenants_plans() -> Result<(), Box<dyn Error>> {
    let tenant_a = TenantId::new(Uuid::new_v4());
    let tenant_b = TenantId::new(Uuid::new_v4());
    let state = app_state(StubReads {
        plans: vec![
            plan(tenant_a, "a-starter", 1999),
            plan(tenant_a, "a-pro", 4999),
            plan(tenant_b, "b-only", 9999),
        ],
    });

    let (status_a, body_a) = get_plans(state.clone(), &tenant_a.as_uuid().to_string()).await?;
    assert_eq!(status_a, StatusCode::OK);
    let names_a: Vec<&str> = body_a
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert_eq!(names_a, ["a-starter", "a-pro"]);

    let (status_b, body_b) = get_plans(state, &tenant_b.as_uuid().to_string()).await?;
    assert_eq!(status_b, StatusCode::OK);
    let names_b: Vec<&str> = body_b
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert_eq!(names_b, ["b-only"]);

    assert!(!body_b.to_string().contains("a-starter"));
    assert!(!body_b.to_string().contains("a-pro"));
    Ok(())
}

#[tokio::test]
async fn money_is_minor_units_and_iso_code_never_a_float() -> Result<(), Box<dyn Error>> {
    let tenant = TenantId::new(Uuid::new_v4());
    let state = app_state(StubReads {
        plans: vec![plan(tenant, "starter", 1999)],
    });

    let (status, body) = get_plans(state, &tenant.as_uuid().to_string()).await?;

    assert_eq!(status, StatusCode::OK);
    let amount = &body[0]["amount"];
    assert_eq!(amount["amount_minor"], 1999);
    assert_eq!(amount["currency"], "EUR");
    assert!(amount["amount_minor"].is_i64());
    assert!(!body.to_string().contains("19.99"));
    Ok(())
}

#[tokio::test]
async fn a_tenant_with_no_plans_gets_an_empty_array() -> Result<(), Box<dyn Error>> {
    let known = TenantId::new(Uuid::new_v4());
    let stranger = TenantId::new(Uuid::new_v4());
    let state = app_state(StubReads {
        plans: vec![plan(known, "starter", 1999)],
    });

    let (status, body) = get_plans(state, &stranger.as_uuid().to_string()).await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, Value::Array(vec![]));
    Ok(())
}
