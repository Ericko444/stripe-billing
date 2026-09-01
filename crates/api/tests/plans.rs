//! Router-level tests for `GET /plans`, driven through `billing_router<T>`
//! with a stub tenant extractor. The load-bearing assertion is tenant
//! isolation: each tenant's `GET /plans` returns only its own rows.

use std::error::Error;
use std::sync::Arc;

use api::{ApiError, AppState, billing_router};
use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{Request, StatusCode};
use domain::{
    Currency, DomainError, Money, Plan, PlanId, TenantId, VerifiedEvent, WebhookReceipt,
    WebhookVerifier,
};
use serde_json::Value;
use service::{EventOutcome, Reads, WebhookHandler};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

/// Test tenant extractor: reads the tenant from an `x-tenant` header. Stands
/// in for the host's real (authenticated) extractor -- all `billing_router`
/// needs is a `T` that rejects with [`ApiError`] and converts
/// [`Into<TenantId>`].
struct HeaderTenant(TenantId);

impl<S: Send + Sync> FromRequestParts<S> for HeaderTenant {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let raw = parts
            .headers
            .get("x-tenant")
            .and_then(|value| value.to_str().ok())
            .ok_or(ApiError::from(DomainError::NotFound))?;
        let id = Uuid::parse_str(raw).map_err(|_| ApiError::from(DomainError::NotFound))?;
        Ok(HeaderTenant(TenantId::new(id)))
    }
}

impl From<HeaderTenant> for TenantId {
    fn from(tenant: HeaderTenant) -> Self {
        tenant.0
    }
}

/// A `Reads` that owns a flat plan list and tenant-scopes it on read, the
/// way the real repository's `WHERE tenant_id = $1` does.
struct StubReads {
    plans: Vec<Plan>,
}

#[async_trait]
impl Reads for StubReads {
    async fn list_plans(&self, tenant: TenantId) -> Result<Vec<Plan>, DomainError> {
        Ok(self
            .plans
            .iter()
            .filter(|plan| plan.tenant_id == tenant)
            .cloned()
            .collect())
    }
}

/// The webhook deps `AppState` requires; `GET /plans` never calls either.
struct UnusedVerifier;

#[async_trait]
impl WebhookVerifier for UnusedVerifier {
    async fn verify_and_record(
        &self,
        _payload: &[u8],
        _signature_header: &str,
    ) -> Result<WebhookReceipt, DomainError> {
        Err(DomainError::WebhookVerification)
    }
}

struct UnusedHandler;

#[async_trait]
impl WebhookHandler for UnusedHandler {
    async fn handle(&self, _event: VerifiedEvent) -> Result<EventOutcome, DomainError> {
        Err(DomainError::WebhookVerification)
    }
}

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

fn app_state(plans: Vec<Plan>) -> AppState {
    AppState::new(
        Arc::new(UnusedVerifier),
        Arc::new(UnusedHandler),
        Arc::new(StubReads { plans }),
    )
}

async fn get_plans(state: AppState, tenant: &str) -> Result<(StatusCode, Value), Box<dyn Error>> {
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
    let plans = vec![
        plan(tenant_a, "a-starter", 1999),
        plan(tenant_a, "a-pro", 4999),
        plan(tenant_b, "b-only", 9999),
    ];
    let state = app_state(plans);

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

    // Nothing of tenant A's appears in tenant B's response.
    assert!(!body_b.to_string().contains("a-starter"));
    assert!(!body_b.to_string().contains("a-pro"));
    Ok(())
}

#[tokio::test]
async fn money_is_minor_units_and_iso_code_never_a_float() -> Result<(), Box<dyn Error>> {
    let tenant = TenantId::new(Uuid::new_v4());
    let state = app_state(vec![plan(tenant, "starter", 1999)]);

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
    let state = app_state(vec![plan(known, "starter", 1999)]);

    let (status, body) = get_plans(state, &stranger.as_uuid().to_string()).await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, Value::Array(vec![]));
    Ok(())
}
