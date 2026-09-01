//! Router-level tests for the Phase 4c write routes, driven through
//! `billing_router<T>` with a stub tenant extractor and a `StubWrites` fake.
//!
//! Task 7 covers `POST /payment-methods/setup-intent`: it answers 200 with a
//! `client_secret`, it resolves the tenant's customer first (a tenant with
//! none still succeeds), that secret never reaches a log line, and two
//! tenants get intents for their own customers.
//!
//! Tasks 8-9 cover `POST /subscriptions/{id}/change-plan` and
//! `POST /subscriptions/{id}/cancel`: malformed path/body ids 404 the same
//! way an unknown or cross-tenant subscription does, and the latter never
//! reaches `Writes` at all; `cancel`'s `at_period_end` defaults to `true`
//! when absent; cancelling an already-canceled subscription is a 200 with no
//! second call.
//!
//! Task 13 covers `POST /payment-methods/{id}/default` (200, updated card)
//! and `DELETE /payment-methods/{id}` (204): a malformed or cross-tenant id
//! is a 404 that never reaches `Writes`, and a second delete of the same id
//! is a 404.

mod common;

use std::error::Error;
use std::sync::Arc;

use api::billing_router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use common::{CapturedLogs, HeaderTenant, StubWrites, app_state_writes};
use domain::{
    CustomerId, PaymentMethod, PaymentMethodId, PlanId, Subscription, SubscriptionId,
    SubscriptionStatus, TenantId,
};
use serde_json::{Value, json};
use time::OffsetDateTime;
use tower::ServiceExt;
use tracing_subscriber::fmt;
use uuid::Uuid;

fn subscription(tenant: TenantId, plan_id: PlanId, status: SubscriptionStatus) -> Subscription {
    let now = OffsetDateTime::now_utc();
    Subscription {
        id: SubscriptionId::new(Uuid::new_v4()),
        tenant_id: tenant,
        customer_id: CustomerId::new(Uuid::new_v4()),
        plan_id,
        stripe_subscription_id: format!("sub_{}", Uuid::new_v4()),
        stripe_subscription_item_id: "si_test".to_string(),
        status,
        current_period_start: now,
        current_period_end: now,
        cancel_at_period_end: false,
        last_event_created_at: None,
        created_at: now,
        deleted_at: None,
    }
}

fn payment_method(tenant: TenantId, is_default: bool) -> PaymentMethod {
    PaymentMethod {
        id: PaymentMethodId::new(Uuid::new_v4()),
        tenant_id: tenant,
        customer_id: CustomerId::new(Uuid::new_v4()),
        stripe_payment_method_id: format!("pm_{}", Uuid::new_v4()),
        brand: "visa".to_string(),
        last4: "4242".to_string(),
        is_default,
        last_event_created_at: None,
        created_at: OffsetDateTime::now_utc(),
        deleted_at: None,
    }
}

async fn send(
    state: api::AppState,
    request: Request<Body>,
) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let response = billing_router::<HeaderTenant>(state)
        .oneshot(request)
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Ok((status, body))
}

async fn post_json(
    state: api::AppState,
    tenant: &str,
    path: &str,
    body: Value,
) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let response = billing_router::<HeaderTenant>(state)
        .oneshot(
            Request::post(path)
                .header("x-tenant", tenant)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body)?))?,
        )
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Ok((status, body))
}

async fn post_setup_intent(
    state: api::AppState,
    tenant: &str,
) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let response = billing_router::<HeaderTenant>(state)
        .oneshot(
            Request::post("/payment-methods/setup-intent")
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
async fn returns_a_client_secret_with_200() -> Result<(), Box<dyn Error>> {
    let tenant = Uuid::new_v4().to_string();
    let state = app_state_writes(Arc::new(StubWrites::default()));

    let (status, body) = post_setup_intent(state, &tenant).await?;

    assert_eq!(status, StatusCode::OK);
    let secret = body["client_secret"].as_str().unwrap_or_default();
    assert!(
        secret.starts_with("seti_") && secret.contains("_secret_"),
        "expected a SetupIntent client_secret, got {body}"
    );
    Ok(())
}

#[tokio::test]
async fn succeeds_for_a_tenant_with_no_customer() -> Result<(), Box<dyn Error>> {
    let tenant = Uuid::new_v4().to_string();
    let writes = Arc::new(StubWrites::default());
    let state = app_state_writes(writes.clone());

    let (status, _body) = post_setup_intent(state, &tenant).await?;

    assert_eq!(status, StatusCode::OK);
    // The route resolved a customer first, even though the tenant had none.
    assert_eq!(writes.setup_intent_calls().len(), 1);
    Ok(())
}

#[tokio::test]
async fn never_logs_the_client_secret() -> Result<(), Box<dyn Error>> {
    let tenant = Uuid::new_v4().to_string();
    let state = app_state_writes(Arc::new(StubWrites::default()));

    let logs = CapturedLogs::default();
    let subscriber = fmt()
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let (status, body) = {
        let _guard = tracing::subscriber::set_default(subscriber);
        post_setup_intent(state, &tenant).await?
    };

    assert_eq!(status, StatusCode::OK);
    let secret = body["client_secret"].as_str().unwrap_or_default();
    assert!(!secret.is_empty());
    let logged = logs.contents();
    assert!(
        !logged.contains(secret),
        "the client_secret must never reach a log line; captured: {logged}"
    );
    assert!(
        !logged.contains("_secret_"),
        "no `seti_..._secret_...` fragment may reach a log line; captured: {logged}"
    );
    Ok(())
}

#[tokio::test]
async fn two_tenants_get_intents_for_their_own_customers() -> Result<(), Box<dyn Error>> {
    let tenant_a = Uuid::new_v4().to_string();
    let tenant_b = Uuid::new_v4().to_string();
    let writes = Arc::new(StubWrites::default());

    let (status_a, body_a) = post_setup_intent(app_state_writes(writes.clone()), &tenant_a).await?;
    let (status_b, body_b) = post_setup_intent(app_state_writes(writes.clone()), &tenant_b).await?;

    assert_eq!(status_a, StatusCode::OK);
    assert_eq!(status_b, StatusCode::OK);

    let secret_a = body_a["client_secret"].as_str().unwrap_or_default();
    let secret_b = body_b["client_secret"].as_str().unwrap_or_default();
    assert_ne!(secret_a, secret_b, "each tenant gets its own client_secret");

    // Each intent named that tenant's own customer, and the two customers
    // differ.
    let calls = writes.setup_intent_calls();
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].1, calls[1].1, "two distinct Stripe customers");
    assert!(secret_a.contains(&calls[0].1));
    assert!(secret_b.contains(&calls[1].1));
    Ok(())
}

// --- Task 8: POST /subscriptions/{id}/change-plan ---------------------

#[tokio::test]
async fn change_plan_returns_the_updated_subscription() -> Result<(), Box<dyn Error>> {
    let tenant_id = TenantId::new(Uuid::new_v4());
    let tenant = tenant_id.as_uuid().to_string();
    let sub = subscription(
        tenant_id,
        PlanId::new(Uuid::new_v4()),
        SubscriptionStatus::PastDue,
    );
    let sub_id = sub.id.as_uuid().to_string();
    let new_plan_id = Uuid::new_v4();
    let writes = Arc::new(StubWrites::default());
    writes.seed_subscription(sub);

    let (status, body) = post_json(
        app_state_writes(writes.clone()),
        &tenant,
        &format!("/subscriptions/{sub_id}/change-plan"),
        json!({ "plan_id": new_plan_id.to_string() }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "active");
    assert_eq!(body["plan_id"], new_plan_id.to_string());
    Ok(())
}

#[tokio::test]
async fn change_plan_with_a_malformed_subscription_id_is_404() -> Result<(), Box<dyn Error>> {
    let tenant = Uuid::new_v4().to_string();

    let (status, _body) = post_json(
        app_state_writes(Arc::new(StubWrites::default())),
        &tenant,
        "/subscriptions/not-a-uuid/change-plan",
        json!({ "plan_id": Uuid::new_v4().to_string() }),
    )
    .await?;

    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn change_plan_with_a_malformed_plan_id_is_404() -> Result<(), Box<dyn Error>> {
    let tenant_id = TenantId::new(Uuid::new_v4());
    let tenant = tenant_id.as_uuid().to_string();
    let sub = subscription(
        tenant_id,
        PlanId::new(Uuid::new_v4()),
        SubscriptionStatus::Active,
    );
    let sub_id = sub.id.as_uuid().to_string();
    let writes = Arc::new(StubWrites::default());
    writes.seed_subscription(sub);

    let (status, _body) = post_json(
        app_state_writes(writes),
        &tenant,
        &format!("/subscriptions/{sub_id}/change-plan"),
        json!({ "plan_id": "not-a-uuid" }),
    )
    .await?;

    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn change_plan_for_another_tenants_subscription_is_404_and_never_reached()
-> Result<(), Box<dyn Error>> {
    let owner = TenantId::new(Uuid::new_v4());
    let caller = TenantId::new(Uuid::new_v4()).as_uuid().to_string();
    let sub = subscription(
        owner,
        PlanId::new(Uuid::new_v4()),
        SubscriptionStatus::Active,
    );
    let sub_id = sub.id.as_uuid().to_string();
    let writes = Arc::new(StubWrites::default());
    writes.seed_subscription(sub);

    let (status, _body) = post_json(
        app_state_writes(writes.clone()),
        &caller,
        &format!("/subscriptions/{sub_id}/change-plan"),
        json!({ "plan_id": Uuid::new_v4().to_string() }),
    )
    .await?;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(writes.change_plan_calls().is_empty());
    Ok(())
}

// --- Task 9: POST /subscriptions/{id}/cancel ---------------------------

#[tokio::test]
async fn cancel_at_period_end_true_sets_the_flag_without_canceling() -> Result<(), Box<dyn Error>> {
    let tenant_id = TenantId::new(Uuid::new_v4());
    let tenant = tenant_id.as_uuid().to_string();
    let sub = subscription(
        tenant_id,
        PlanId::new(Uuid::new_v4()),
        SubscriptionStatus::Active,
    );
    let sub_id = sub.id.as_uuid().to_string();
    let writes = Arc::new(StubWrites::default());
    writes.seed_subscription(sub);

    let (status, body) = post_json(
        app_state_writes(writes),
        &tenant,
        &format!("/subscriptions/{sub_id}/cancel"),
        json!({ "at_period_end": true }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["cancel_at_period_end"], true);
    assert_eq!(body["status"], "active");
    Ok(())
}

#[tokio::test]
async fn absent_at_period_end_defaults_to_true() -> Result<(), Box<dyn Error>> {
    let tenant_id = TenantId::new(Uuid::new_v4());
    let tenant = tenant_id.as_uuid().to_string();
    let sub = subscription(
        tenant_id,
        PlanId::new(Uuid::new_v4()),
        SubscriptionStatus::Active,
    );
    let sub_id = sub.id.as_uuid().to_string();
    let writes = Arc::new(StubWrites::default());
    writes.seed_subscription(sub);

    let (status, body) = post_json(
        app_state_writes(writes),
        &tenant,
        &format!("/subscriptions/{sub_id}/cancel"),
        json!({}),
    )
    .await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["cancel_at_period_end"], true,
        "an absent at_period_end must default to the safer `true`, not bool::default()'s false"
    );
    Ok(())
}

#[tokio::test]
async fn at_period_end_false_cancels_immediately() -> Result<(), Box<dyn Error>> {
    let tenant_id = TenantId::new(Uuid::new_v4());
    let tenant = tenant_id.as_uuid().to_string();
    let sub = subscription(
        tenant_id,
        PlanId::new(Uuid::new_v4()),
        SubscriptionStatus::Active,
    );
    let sub_id = sub.id.as_uuid().to_string();
    let writes = Arc::new(StubWrites::default());
    writes.seed_subscription(sub);

    let (status, body) = post_json(
        app_state_writes(writes),
        &tenant,
        &format!("/subscriptions/{sub_id}/cancel"),
        json!({ "at_period_end": false }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "canceled");
    Ok(())
}

#[tokio::test]
async fn cancel_for_another_tenants_subscription_is_404_and_never_reached()
-> Result<(), Box<dyn Error>> {
    let owner = TenantId::new(Uuid::new_v4());
    let caller = TenantId::new(Uuid::new_v4()).as_uuid().to_string();
    let sub = subscription(
        owner,
        PlanId::new(Uuid::new_v4()),
        SubscriptionStatus::Active,
    );
    let sub_id = sub.id.as_uuid().to_string();
    let writes = Arc::new(StubWrites::default());
    writes.seed_subscription(sub);

    let (status, _body) = post_json(
        app_state_writes(writes.clone()),
        &caller,
        &format!("/subscriptions/{sub_id}/cancel"),
        json!({ "at_period_end": true }),
    )
    .await?;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(writes.cancel_calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn cancelling_an_already_canceled_subscription_is_200_with_no_second_call()
-> Result<(), Box<dyn Error>> {
    let tenant_id = TenantId::new(Uuid::new_v4());
    let tenant = tenant_id.as_uuid().to_string();
    let sub = subscription(
        tenant_id,
        PlanId::new(Uuid::new_v4()),
        SubscriptionStatus::Canceled,
    );
    let sub_id = sub.id.as_uuid().to_string();
    let writes = Arc::new(StubWrites::default());
    writes.seed_subscription(sub);

    let (status, body) = post_json(
        app_state_writes(writes.clone()),
        &tenant,
        &format!("/subscriptions/{sub_id}/cancel"),
        json!({ "at_period_end": true }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "canceled");
    assert!(
        writes.cancel_calls().is_empty(),
        "an already-canceled subscription must not place a second call"
    );
    Ok(())
}

// --- Task 13: POST /payment-methods/{id}/default -----------------------

#[tokio::test]
async fn set_default_returns_the_updated_card() -> Result<(), Box<dyn Error>> {
    let tenant_id = TenantId::new(Uuid::new_v4());
    let tenant = tenant_id.as_uuid().to_string();
    let old_default = payment_method(tenant_id, true);
    let mut target = payment_method(tenant_id, false);
    target.customer_id = old_default.customer_id;
    let target_id = target.id.as_uuid().to_string();
    let writes = Arc::new(StubWrites::default());
    writes.seed_payment_method(old_default);
    writes.seed_payment_method(target);

    let (status, body) = send(
        app_state_writes(writes),
        Request::post(format!("/payment-methods/{target_id}/default"))
            .header("x-tenant", &tenant)
            .body(Body::empty())?,
    )
    .await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], target_id);
    assert_eq!(body["is_default"], true);
    Ok(())
}

#[tokio::test]
async fn set_default_with_a_malformed_id_is_404() -> Result<(), Box<dyn Error>> {
    let tenant = Uuid::new_v4().to_string();

    let (status, _body) = send(
        app_state_writes(Arc::new(StubWrites::default())),
        Request::post("/payment-methods/not-a-uuid/default")
            .header("x-tenant", &tenant)
            .body(Body::empty())?,
    )
    .await?;

    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn set_default_for_another_tenants_card_is_404_and_never_reached()
-> Result<(), Box<dyn Error>> {
    let owner = TenantId::new(Uuid::new_v4());
    let caller = TenantId::new(Uuid::new_v4()).as_uuid().to_string();
    let card = payment_method(owner, false);
    let card_id = card.id.as_uuid().to_string();
    let writes = Arc::new(StubWrites::default());
    writes.seed_payment_method(card);

    let (status, _body) = send(
        app_state_writes(writes.clone()),
        Request::post(format!("/payment-methods/{card_id}/default"))
            .header("x-tenant", &caller)
            .body(Body::empty())?,
    )
    .await?;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(writes.set_default_calls().is_empty());
    Ok(())
}

// --- Task 13: DELETE /payment-methods/{id} ---------------------------

#[tokio::test]
async fn delete_returns_204_and_records_the_removal() -> Result<(), Box<dyn Error>> {
    let tenant_id = TenantId::new(Uuid::new_v4());
    let tenant = tenant_id.as_uuid().to_string();
    let card = payment_method(tenant_id, false);
    let card_id = card.id;
    let card_id_str = card_id.as_uuid().to_string();
    let writes = Arc::new(StubWrites::default());
    writes.seed_payment_method(card);

    let (status, body) = send(
        app_state_writes(writes.clone()),
        Request::delete(format!("/payment-methods/{card_id_str}"))
            .header("x-tenant", &tenant)
            .body(Body::empty())?,
    )
    .await?;

    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null, "204 carries no body");
    assert_eq!(writes.remove_calls(), vec![(tenant_id, card_id)]);
    Ok(())
}

#[tokio::test]
async fn delete_with_a_malformed_id_is_404() -> Result<(), Box<dyn Error>> {
    let tenant = Uuid::new_v4().to_string();

    let (status, _body) = send(
        app_state_writes(Arc::new(StubWrites::default())),
        Request::delete("/payment-methods/not-a-uuid")
            .header("x-tenant", &tenant)
            .body(Body::empty())?,
    )
    .await?;

    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn delete_for_another_tenants_card_is_404_and_never_reached() -> Result<(), Box<dyn Error>> {
    let owner = TenantId::new(Uuid::new_v4());
    let caller = TenantId::new(Uuid::new_v4()).as_uuid().to_string();
    let card = payment_method(owner, false);
    let card_id = card.id.as_uuid().to_string();
    let writes = Arc::new(StubWrites::default());
    writes.seed_payment_method(card);

    let (status, _body) = send(
        app_state_writes(writes.clone()),
        Request::delete(format!("/payment-methods/{card_id}"))
            .header("x-tenant", &caller)
            .body(Body::empty())?,
    )
    .await?;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(writes.remove_calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn deleting_an_already_removed_card_is_404() -> Result<(), Box<dyn Error>> {
    let tenant_id = TenantId::new(Uuid::new_v4());
    let tenant = tenant_id.as_uuid().to_string();
    let card = payment_method(tenant_id, false);
    let card_id = card.id.as_uuid().to_string();
    let writes = Arc::new(StubWrites::default());
    writes.seed_payment_method(card);
    let state = || app_state_writes(writes.clone());

    let (first, _) = send(
        state(),
        Request::delete(format!("/payment-methods/{card_id}"))
            .header("x-tenant", &tenant)
            .body(Body::empty())?,
    )
    .await?;
    assert_eq!(first, StatusCode::NO_CONTENT);

    let (second, _) = send(
        state(),
        Request::delete(format!("/payment-methods/{card_id}"))
            .header("x-tenant", &tenant)
            .body(Body::empty())?,
    )
    .await?;
    assert_eq!(second, StatusCode::NOT_FOUND, "the row is already gone");
    Ok(())
}
