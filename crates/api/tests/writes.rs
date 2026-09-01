//! Router-level tests for the Phase 4c write routes, driven through
//! `billing_router<T>` with a stub tenant extractor and a `StubWrites` fake.
//!
//! Task 7 covers `POST /payment-methods/setup-intent`: it answers 200 with a
//! `client_secret`, it resolves the tenant's customer first (a tenant with
//! none still succeeds), that secret never reaches a log line, and two
//! tenants get intents for their own customers.

mod common;

use std::error::Error;
use std::sync::Arc;

use api::billing_router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use common::{CapturedLogs, HeaderTenant, StubWrites, app_state_writes};
use serde_json::Value;
use tower::ServiceExt;
use tracing_subscriber::fmt;
use uuid::Uuid;

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
