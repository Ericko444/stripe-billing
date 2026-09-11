//! The whole-surface tenant-isolation suite.
//!
//! Seeds two tenants with different data across every mirror table, drives
//! **all eleven** tenant-scoped routes -- five read, six write -- as each,
//! and asserts two things the write routes make sharper than the read ones:
//!
//! - no response ever contains an id belonging to the *other* tenant
//!   (a read leak is a disclosure; a write leak would be a mutation on data
//!   the caller does not own); and
//! - for every write route, a **cross-tenant id makes zero provider calls**
//!   -- recorded by the `StubWrites` double, not inferred from the response.
//!
//! Plus the negative tests for the usual ways isolation is lost (a
//! `?tenant_id=` parameter, a `tenant_id` in a JSON body), a check that
//! `POST /webhooks/stripe` still answers with no tenant context at all, and
//! that every error in the suite is `application/problem+json`.
//!
//! Every earlier task has a two-tenant test for its own route. This file
//! walks the whole surface once, which is what catches the route someone
//! adds later and forgets to scope.

mod common;

use std::error::Error;
use std::sync::Arc;

use api::{AppState, billing_router, webhook_router};
use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use common::{HeaderTenant, StubReads, StubWrites, UnusedHandler, UnusedWrites};
use domain::{
    Currency, CustomerId, DomainError, Invoice, InvoiceId, InvoiceStatus, Money, PaymentMethod,
    PaymentMethodId, Plan, PlanId, Subscription, SubscriptionId, SubscriptionStatus, TenantId,
    WebhookReceipt, WebhookVerifier,
};
use serde_json::{Value, json};
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

fn read_seed(
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

/// A `StubWrites` seeded with both tenants' subscriptions, payment methods
/// and plans, so the write routes' tenant-scoped lookups resolve.
fn seeded_writes(a: &Tenant, b: &Tenant) -> Arc<StubWrites> {
    let writes = Arc::new(StubWrites::default());
    for t in [a, b] {
        writes.seed_subscription(subscription(t.id, t.subscription_id, t.plan_id));
        writes.seed_payment_method(payment_method(t.id, t.payment_method_id));
        writes.seed_plan(plan(t.id, t.plan_id, "starter"));
    }
    writes
}

/// One `AppState` with both a real read stub and a real write stub, plus the
/// `StubWrites` handle so a test can assert on `*_calls()`.
fn two_tenant_state(a: &Tenant, b: &Tenant) -> (AppState, Arc<StubWrites>) {
    let (mut plans, mut subs, mut pms, mut invoices) = read_seed(a);
    let (bp, bs, bpm, bi) = read_seed(b);
    plans.extend(bp);
    subs.extend(bs);
    pms.extend(bpm);
    invoices.extend(bi);
    let writes = seeded_writes(a, b);
    let state = common::app_state_full(
        StubReads {
            plans,
            subscriptions: subs,
            payment_methods: pms,
            invoices,
        },
        writes.clone(),
    );
    (state, writes)
}

// --- request helpers -----------------------------------------------------

async fn send(
    state: AppState,
    request: Request<Body>,
) -> Result<(StatusCode, Option<String>, Value), Box<dyn Error>> {
    let response = billing_router::<HeaderTenant>(state)
        .oneshot(request)
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
    send(state, request.body(body)?).await
}

async fn post(
    state: AppState,
    path: &str,
    tenant: &str,
    body: Value,
) -> Result<(StatusCode, Option<String>, Value), Box<dyn Error>> {
    let request = Request::post(path)
        .header("x-tenant", tenant)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body)?))?;
    send(state, request).await
}

async fn delete(
    state: AppState,
    path: &str,
    tenant: &str,
) -> Result<(StatusCode, Option<String>, Value), Box<dyn Error>> {
    let request = Request::delete(path)
        .header("x-tenant", tenant)
        .body(Body::empty())?;
    send(state, request).await
}

/// Every read route plus `GET /invoices/{own id}`, as one concatenated JSON
/// blob for substring assertions.
async fn read_route_bodies(state: AppState, t: &Tenant) -> Result<String, Box<dyn Error>> {
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

/// Every **write** route, driven as `t` with `t`'s own ids, as one blob. The
/// mutations land only on `t`'s own rows in the shared `StubWrites`.
async fn write_route_bodies(state: AppState, t: &Tenant) -> Result<String, Box<dyn Error>> {
    let header = t.header();
    let sub = t.subscription_id.as_uuid();
    let pm = t.payment_method_id.as_uuid();
    let plan = t.plan_id.as_uuid().to_string();
    let mut blob = String::new();

    let (s, _, body) = post(
        state.clone(),
        "/payment-methods/setup-intent",
        &header,
        json!({}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK, "setup-intent for its own tenant is 200");
    blob.push_str(&body.to_string());

    let (s, _, body) = post(
        state.clone(),
        &format!("/subscriptions/{sub}/change-plan"),
        &header,
        json!({ "plan_id": plan }),
    )
    .await?;
    assert_eq!(
        s,
        StatusCode::OK,
        "change-plan for its own subscription is 200"
    );
    blob.push_str(&body.to_string());

    let (s, _, body) = post(
        state.clone(),
        &format!("/subscriptions/{sub}/cancel"),
        &header,
        json!({ "at_period_end": true }),
    )
    .await?;
    assert_eq!(s, StatusCode::OK, "cancel for its own subscription is 200");
    blob.push_str(&body.to_string());

    let (s, _, body) = post(
        state.clone(),
        &format!("/payment-methods/{pm}/default"),
        &header,
        json!({}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK, "set-default for its own card is 200");
    blob.push_str(&body.to_string());

    let (s, _, body) = post(
        state.clone(),
        "/subscriptions/checkout-session",
        &header,
        json!({ "plan_id": plan }),
    )
    .await?;
    assert_eq!(
        s,
        StatusCode::OK,
        "checkout-session for its own plan is 200"
    );
    blob.push_str(&body.to_string());

    // DELETE is 204 with no body -- nothing to blob, but it must succeed for
    // the tenant's own card, and it is done last so it does not remove the
    // row the set-default call above needs.
    let (s, _, _) = delete(state, &format!("/payment-methods/{pm}"), &header).await?;
    assert_eq!(s, StatusCode::NO_CONTENT, "DELETE own card is 204");

    Ok(blob)
}

// --- the suite ---------------------------------------------------------

#[tokio::test]
async fn every_route_returns_only_the_calling_tenants_data() -> Result<(), Box<dyn Error>> {
    let a = Tenant::new();
    let b = Tenant::new();
    let (state, _writes) = two_tenant_state(&a, &b);

    let a_bodies = format!(
        "{}{}",
        read_route_bodies(state.clone(), &a).await?,
        write_route_bodies(state.clone(), &a).await?,
    );
    let b_bodies = format!(
        "{}{}",
        read_route_bodies(state.clone(), &b).await?,
        write_route_bodies(state, &b).await?,
    );

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

/// Asserts a write route returned a `problem+json` 404 that carries none of
/// the other tenant's ids.
fn assert_cross_tenant_404(label: &str, resp: (StatusCode, Option<String>, Value), other: &Tenant) {
    let (status, content_type, body) = resp;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "{label}: cross-tenant id is a 404"
    );
    assert_eq!(
        content_type.as_deref(),
        Some("application/problem+json"),
        "{label}: the 404 is problem+json"
    );
    let rendered = body.to_string();
    for id in other.id_strings() {
        assert!(
            !rendered.contains(&id),
            "{label}: {id} leaked into the 404 body"
        );
    }
}

#[tokio::test]
async fn every_write_route_rejects_a_cross_tenant_id_without_an_outbound_call()
-> Result<(), Box<dyn Error>> {
    let a = Tenant::new();
    let b = Tenant::new();
    let (state, writes) = two_tenant_state(&a, &b);
    let a_header = a.header();

    // A, aiming every write route at B's id.
    let b_sub = b.subscription_id.as_uuid();
    let b_pm = b.payment_method_id.as_uuid();
    let b_plan = b.plan_id.as_uuid().to_string();
    let a_plan = a.plan_id.as_uuid().to_string();

    assert_cross_tenant_404(
        "change-plan (B's subscription)",
        post(
            state.clone(),
            &format!("/subscriptions/{b_sub}/change-plan"),
            &a_header,
            json!({ "plan_id": a_plan }),
        )
        .await?,
        &b,
    );
    assert_cross_tenant_404(
        "cancel (B's subscription)",
        post(
            state.clone(),
            &format!("/subscriptions/{b_sub}/cancel"),
            &a_header,
            json!({ "at_period_end": true }),
        )
        .await?,
        &b,
    );
    assert_cross_tenant_404(
        "set-default (B's card)",
        post(
            state.clone(),
            &format!("/payment-methods/{b_pm}/default"),
            &a_header,
            json!({}),
        )
        .await?,
        &b,
    );
    assert_cross_tenant_404(
        "delete (B's card)",
        delete(
            state.clone(),
            &format!("/payment-methods/{b_pm}"),
            &a_header,
        )
        .await?,
        &b,
    );
    assert_cross_tenant_404(
        "checkout-session (B's plan)",
        post(
            state.clone(),
            "/subscriptions/checkout-session",
            &a_header,
            json!({ "plan_id": b_plan }),
        )
        .await?,
        &b,
    );

    // The load-bearing half: not one of those calls reached the provider on
    // B's behalf.
    assert!(
        writes.change_plan_calls().is_empty(),
        "change_plan reached the provider"
    );
    assert!(
        writes.cancel_calls().is_empty(),
        "cancel reached the provider"
    );
    assert!(
        writes.set_default_calls().is_empty(),
        "set_default reached the provider"
    );
    assert!(
        writes.remove_calls().is_empty(),
        "remove reached the provider"
    );
    assert!(
        writes.checkout_calls().is_empty(),
        "checkout reached the provider"
    );
    Ok(())
}

#[tokio::test]
async fn setup_intent_is_for_the_calling_tenants_own_customer() -> Result<(), Box<dyn Error>> {
    let a = Tenant::new();
    let b = Tenant::new();
    let (state, _writes) = two_tenant_state(&a, &b);

    let (status_a, _, body_a) = post(
        state.clone(),
        "/payment-methods/setup-intent",
        &a.header(),
        json!({}),
    )
    .await?;
    let (status_b, _, body_b) = post(
        state,
        "/payment-methods/setup-intent",
        &b.header(),
        json!({}),
    )
    .await?;

    assert_eq!(status_a, StatusCode::OK);
    assert_eq!(status_b, StatusCode::OK);
    let secret_a = body_a["client_secret"].as_str().unwrap_or_default();
    let secret_b = body_b["client_secret"].as_str().unwrap_or_default();
    assert_ne!(
        secret_a, secret_b,
        "each tenant's SetupIntent is for its own customer"
    );
    // A's secret is derived from A's tenant id; B's must not appear in it.
    assert!(!secret_a.contains(&b.id.as_uuid().simple().to_string()));
    Ok(())
}

#[tokio::test]
async fn another_tenants_invoice_id_is_a_404_not_a_leak() -> Result<(), Box<dyn Error>> {
    let a = Tenant::new();
    let b = Tenant::new();
    let (state, _writes) = two_tenant_state(&a, &b);

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
    let (state, _writes) = two_tenant_state(&a, &b);

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
    let (state, _writes) = two_tenant_state(&a, &b);

    // A GET with a spoofed body (read path), and a POST checkout-session with
    // an extra `tenant_id` key alongside A's own plan (write path). Neither
    // DTO uses `deny_unknown_fields`, so the extra key must be inert.
    let read_body = Body::from(format!(r#"{{"tenant_id":"{}"}}"#, b.id.as_uuid()));
    let (status, _, response) = get(state.clone(), "/plans", Some(&a.header()), read_body).await?;
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

    let (status, _, response) = post(
        state,
        "/subscriptions/checkout-session",
        &a.header(),
        json!({ "plan_id": a.plan_id.as_uuid().to_string(), "tenant_id": b.id.as_uuid().to_string() }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "the spoofed tenant_id key is inert");
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
        common::checkout_urls(),
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
    let (state, _writes) = two_tenant_state(&a, &b);

    // Unknown invoice id -> 404 (read path).
    let (status, ct, _) = get(
        state.clone(),
        &format!("/invoices/{}", Uuid::new_v4()),
        Some(&a.header()),
        Body::empty(),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(ct.as_deref(), Some("application/problem+json"));

    // Malformed cursor -> 400 (read path).
    let (status, ct, _) = get(
        state.clone(),
        "/invoices?after=not-a-cursor",
        Some(&a.header()),
        Body::empty(),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(ct.as_deref(), Some("application/problem+json"));

    // Unknown subscription id on a write route -> 404 (write path).
    let (status, ct, _) = post(
        state.clone(),
        &format!("/subscriptions/{}/cancel", Uuid::new_v4()),
        &a.header(),
        json!({ "at_period_end": true }),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(ct.as_deref(), Some("application/problem+json"));

    // No tenant header at all -> the extractor rejects with an ApiError.
    let (status, ct, _) = get(state, "/plans", None, Body::empty()).await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(ct.as_deref(), Some("application/problem+json"));
    Ok(())
}
