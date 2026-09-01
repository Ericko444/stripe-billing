//! Phase 4b's deliverable: one suite that seeds two tenants with different
//! data across every mirror table, drives **all five** read routes as each,
//! and asserts nothing of one tenant's ever reaches the other -- plus the
//! negative tests for the ways tenant isolation is usually lost (a
//! `?tenant_id=` parameter, a tenant id in a body) and a check that the
//! webhook route still answers with no tenant context at all.
//!
//! Every earlier task has a two-tenant test for its own route. This one
//! walks the whole surface in a single file, which is what catches the route
//! someone adds later and forgets to scope.

mod common;

use std::error::Error;
use std::sync::Arc;

use api::{AppState, billing_router, webhook_router};
use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use common::{HeaderTenant, StubReads, UnusedHandler, UnusedWrites};
use domain::{
    Currency, CustomerId, DomainError, Invoice, InvoiceId, InvoiceStatus, Money, PaymentMethod,
    PaymentMethodId, Plan, PlanId, Subscription, SubscriptionId, SubscriptionStatus, TenantId,
    WebhookReceipt, WebhookVerifier,
};
use serde_json::Value;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

// --- builders --------------------------------------------------------------

fn plan(tenant: TenantId, id: PlanId, name: &str) -> Plan {
    Plan {
        id,
        tenant_id: tenant,
        stripe_price_id: format!("price_{name}"),
        stripe_product_id: format!("prod_{name}"),
        name: name.to_string(),
        amount: Money::new(1999, Currency::Eur),
        created_at: OffsetDateTime::UNIX_EPOCH,
        deleted_at: None,
    }
}

fn subscription(tenant: TenantId, id: SubscriptionId, plan_id: PlanId) -> Subscription {
    let start = OffsetDateTime::UNIX_EPOCH + Duration::days(10);
    Subscription {
        id,
        tenant_id: tenant,
        customer_id: CustomerId::new(Uuid::new_v4()),
        plan_id,
        stripe_subscription_id: format!("sub_{}", Uuid::new_v4()),
        stripe_subscription_item_id: "si_test".to_string(),
        status: SubscriptionStatus::Active,
        current_period_start: start,
        current_period_end: start + Duration::days(30),
        cancel_at_period_end: false,
        last_event_created_at: None,
        created_at: start,
        deleted_at: None,
    }
}

fn payment_method(tenant: TenantId, id: PaymentMethodId) -> PaymentMethod {
    PaymentMethod {
        id,
        tenant_id: tenant,
        customer_id: CustomerId::new(Uuid::new_v4()),
        stripe_payment_method_id: format!("pm_{}", Uuid::new_v4()),
        brand: "visa".to_string(),
        last4: "4242".to_string(),
        is_default: true,
        last_event_created_at: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
        deleted_at: None,
    }
}

fn invoice(tenant: TenantId, id: InvoiceId) -> Invoice {
    Invoice {
        id,
        tenant_id: tenant,
        customer_id: CustomerId::new(Uuid::new_v4()),
        subscription_id: None,
        stripe_invoice_id: format!("in_{}", Uuid::new_v4()),
        amount: Money::new(4200, Currency::Eur),
        status: InvoiceStatus::Paid,
        last_event_created_at: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
        deleted_at: None,
    }
}

/// Every id that belongs to one tenant, as hyphenated-uuid strings -- the
/// form they take on the wire.
struct Tenant {
    id: TenantId,
    plan_id: PlanId,
    subscription_id: SubscriptionId,
    payment_method_id: PaymentMethodId,
    invoice_id: InvoiceId,
}

impl Tenant {
    fn new() -> Self {
        Self {
            id: TenantId::new(Uuid::new_v4()),
            plan_id: PlanId::new(Uuid::new_v4()),
            subscription_id: SubscriptionId::new(Uuid::new_v4()),
            payment_method_id: PaymentMethodId::new(Uuid::new_v4()),
            invoice_id: InvoiceId::new(Uuid::new_v4()),
        }
    }

    fn header(&self) -> String {
        self.id.as_uuid().to_string()
    }

    /// The uuid strings a response for the *other* tenant must never contain.
    fn id_strings(&self) -> Vec<String> {
        vec![
            self.plan_id.as_uuid().to_string(),
            self.subscription_id.as_uuid().to_string(),
            self.payment_method_id.as_uuid().to_string(),
            self.invoice_id.as_uuid().to_string(),
        ]
    }
}

fn seed(
    t: &Tenant,
) -> (
    Vec<Plan>,
    Vec<Subscription>,
    Vec<PaymentMethod>,
    Vec<Invoice>,
) {
    (
        vec![plan(t.id, t.plan_id, "starter")],
        vec![subscription(t.id, t.subscription_id, t.plan_id)],
        vec![payment_method(t.id, t.payment_method_id)],
        vec![invoice(t.id, t.invoice_id)],
    )
}

fn two_tenant_state(a: &Tenant, b: &Tenant) -> AppState {
    let (mut plans, mut subs, mut pms, mut invoices) = seed(a);
    let (bp, bs, bpm, bi) = seed(b);
    plans.extend(bp);
    subs.extend(bs);
    pms.extend(bpm);
    invoices.extend(bi);
    common::app_state(StubReads {
        plans,
        subscriptions: subs,
        payment_methods: pms,
        invoices,
    })
}

// --- request helpers -----------------------------------------------------

async fn get(
    state: AppState,
    path: &str,
    tenant_header: Option<&str>,
    body: Body,
) -> Result<(StatusCode, Option<String>, Value), Box<dyn Error>> {
    let mut request = Request::get(path);
    if let Some(tenant) = tenant_header {
        request = request.header("x-tenant", tenant);
    }
    let response = billing_router::<HeaderTenant>(state)
        .oneshot(request.body(body)?)
        .await?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Ok((status, content_type, value))
}

/// The four collection/singleton routes plus `GET /invoices/{own id}`, as
/// one concatenated JSON blob for substring assertions.
async fn all_route_bodies(state: AppState, t: &Tenant) -> Result<String, Box<dyn Error>> {
    let header = t.header();
    let mut blob = String::new();
    for path in ["/plans", "/subscription", "/payment-methods", "/invoices"] {
        let (status, _, body) = get(state.clone(), path, Some(&header), Body::empty()).await?;
        assert_eq!(status, StatusCode::OK, "{path} for its own tenant is 200");
        blob.push_str(&body.to_string());
    }
    let (status, _, body) = get(
        state,
        &format!("/invoices/{}", t.invoice_id.as_uuid()),
        Some(&header),
        Body::empty(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "GET /invoices/{{own id}} is 200");
    blob.push_str(&body.to_string());
    Ok(blob)
}

// --- the suite ---------------------------------------------------------

#[tokio::test]
async fn every_route_returns_only_the_calling_tenants_data() -> Result<(), Box<dyn Error>> {
    let a = Tenant::new();
    let b = Tenant::new();
    let state = two_tenant_state(&a, &b);

    let a_bodies = all_route_bodies(state.clone(), &a).await?;
    let b_bodies = all_route_bodies(state, &b).await?;

    // Each tenant's own ids show up (the suite is not passing vacuously).
    for id in a.id_strings() {
        assert!(
            a_bodies.contains(&id),
            "tenant A's {id} missing from A's own responses"
        );
    }
    // And none of the other tenant's ids appear anywhere.
    for id in b.id_strings() {
        assert!(
            !a_bodies.contains(&id),
            "tenant B's {id} leaked into A's responses"
        );
    }
    for id in a.id_strings() {
        assert!(
            !b_bodies.contains(&id),
            "tenant A's {id} leaked into B's responses"
        );
    }
    Ok(())
}

#[tokio::test]
async fn another_tenants_invoice_id_is_a_404_not_a_leak() -> Result<(), Box<dyn Error>> {
    let a = Tenant::new();
    let b = Tenant::new();
    let state = two_tenant_state(&a, &b);

    let (status, content_type, body) = get(
        state,
        &format!("/invoices/{}", b.invoice_id.as_uuid()),
        Some(&a.header()),
        Body::empty(),
    )
    .await?;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    assert!(
        !body
            .to_string()
            .contains(&b.invoice_id.as_uuid().to_string())
    );
    Ok(())
}

#[tokio::test]
async fn a_tenant_id_query_parameter_is_ignored() -> Result<(), Box<dyn Error>> {
    let a = Tenant::new();
    let b = Tenant::new();
    let state = two_tenant_state(&a, &b);

    // Calling as A but asking for B via the query string.
    let (status, _, body) = get(
        state,
        &format!("/plans?tenant_id={}", b.id.as_uuid()),
        Some(&a.header()),
        Body::empty(),
    )
    .await?;

    assert_eq!(status, StatusCode::OK);
    assert!(body.to_string().contains(&a.plan_id.as_uuid().to_string()));
    assert!(!body.to_string().contains(&b.plan_id.as_uuid().to_string()));
    Ok(())
}

#[tokio::test]
async fn a_tenant_id_in_the_request_body_is_ignored() -> Result<(), Box<dyn Error>> {
    let a = Tenant::new();
    let b = Tenant::new();
    let state = two_tenant_state(&a, &b);

    let body = Body::from(format!(r#"{{"tenant_id":"{}"}}"#, b.id.as_uuid()));
    let (status, _, response) = get(state, "/plans", Some(&a.header()), body).await?;

    assert_eq!(status, StatusCode::OK);
    assert!(
        response
            .to_string()
            .contains(&a.plan_id.as_uuid().to_string())
    );
    assert!(
        !response
            .to_string()
            .contains(&b.plan_id.as_uuid().to_string())
    );
    Ok(())
}

/// A verifier that reports every delivery as an already-seen duplicate, so
/// `POST /webhooks/stripe` reaches its 200 without a handler call and,
/// crucially, without a tenant anywhere in scope.
struct DuplicateVerifier;

#[async_trait]
impl WebhookVerifier for DuplicateVerifier {
    async fn verify_and_record(
        &self,
        _payload: &[u8],
        _signature_header: &str,
    ) -> Result<WebhookReceipt, DomainError> {
        Ok(WebhookReceipt::Duplicate {
            stripe_event_id: "evt_dup".to_string(),
        })
    }
}

#[tokio::test]
async fn the_webhook_route_answers_with_no_tenant_context() -> Result<(), Box<dyn Error>> {
    let state = AppState::new(
        Arc::new(DuplicateVerifier),
        Arc::new(UnusedHandler),
        Arc::new(StubReads::default()),
        Arc::new(UnusedWrites),
    );

    // Through `webhook_router` -- the non-generic factory. No `x-tenant`
    // header, no tenant extractor type named anywhere in this request.
    let response = webhook_router(state)
        .oneshot(
            Request::post("/webhooks/stripe")
                .header("Stripe-Signature", "t=1,v1=whatever")
                .body(Body::from("{}"))?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn every_error_in_this_suite_is_problem_json() -> Result<(), Box<dyn Error>> {
    let a = Tenant::new();
    let b = Tenant::new();
    let state = two_tenant_state(&a, &b);

    // Unknown invoice id -> 404.
    let (status, ct, _) = get(
        state.clone(),
        &format!("/invoices/{}", Uuid::new_v4()),
        Some(&a.header()),
        Body::empty(),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(ct.as_deref(), Some("application/problem+json"));

    // Malformed cursor -> 400.
    let (status, ct, _) = get(
        state.clone(),
        "/invoices?after=not-a-cursor",
        Some(&a.header()),
        Body::empty(),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(ct.as_deref(), Some("application/problem+json"));

    // No tenant header at all -> the extractor rejects with an ApiError.
    let (status, ct, _) = get(state, "/plans", None, Body::empty()).await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(ct.as_deref(), Some("application/problem+json"));
    Ok(())
}
