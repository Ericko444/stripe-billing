//! Router-level tests for `POST /webhooks/stripe`, driving the router
//! with `tower::ServiceExt::oneshot` (no bound port).

use std::error::Error;
use std::sync::{Arc, Mutex};

use api::{AppState, billing_router};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use domain::{DomainError, VerifiedEvent, WebhookEventId, WebhookReceipt, WebhookVerifier};
use serde_json::json;
use service::{EventOutcome, NotAppliedReason, WebhookHandler};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

/// What [`StubVerifier`] returns, and whether it errors instead.
enum VerifierResponse {
    Fresh(VerifiedEvent),
    Duplicate,
    VerificationFailed,
}

/// Doubles `domain::WebhookVerifier` at the trait, the way the spec's
/// Testing Strategy calls for at this layer. Records the exact payload
/// bytes it was handed, so a test can assert they reached the verifier
/// byte-for-byte -- the guard for §10.1's raw-bytes precondition.
struct StubVerifier {
    response: VerifierResponse,
    captured_payload: Mutex<Option<Vec<u8>>>,
}

impl StubVerifier {
    fn new(response: VerifierResponse) -> Self {
        Self {
            response,
            captured_payload: Mutex::new(None),
        }
    }

    fn captured_payload(&self) -> Option<Vec<u8>> {
        self.captured_payload
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[async_trait]
impl WebhookVerifier for StubVerifier {
    async fn verify_and_record(
        &self,
        payload: &[u8],
        _signature_header: &str,
    ) -> Result<WebhookReceipt, DomainError> {
        *self
            .captured_payload
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(payload.to_vec());

        match &self.response {
            VerifierResponse::Fresh(event) => Ok(WebhookReceipt::Fresh(event.clone())),
            VerifierResponse::Duplicate => Ok(WebhookReceipt::Duplicate {
                stripe_event_id: "evt_duplicate".to_string(),
            }),
            VerifierResponse::VerificationFailed => Err(DomainError::WebhookVerification),
        }
    }
}

/// Doubles `service::WebhookHandler`: always returns the configured
/// outcome. The route's own logic (Fresh/Duplicate handling, status
/// mapping) is what these tests exercise -- what the handler decided is
/// not under test here, so a fixed response is all this needs.
struct StubHandler {
    outcome: EventOutcome,
}

#[async_trait]
impl WebhookHandler for StubHandler {
    async fn handle(&self, _event: VerifiedEvent) -> Result<EventOutcome, DomainError> {
        Ok(self.outcome.clone())
    }
}

fn verified_event() -> VerifiedEvent {
    VerifiedEvent {
        id: WebhookEventId::new(Uuid::new_v4()),
        stripe_event_id: "evt_test".to_string(),
        event_type: "customer.subscription.updated".to_string(),
        created: OffsetDateTime::now_utc(),
        payload: json!({}),
    }
}

fn app_state(verifier: Arc<StubVerifier>, outcome: EventOutcome) -> AppState {
    AppState::new(verifier, Arc::new(StubHandler { outcome }))
}

#[tokio::test]
async fn valid_signature_returns_200() -> Result<(), Box<dyn Error>> {
    let verifier = Arc::new(StubVerifier::new(VerifierResponse::Fresh(verified_event())));
    let router = billing_router(app_state(
        verifier,
        EventOutcome::NotApplied(NotAppliedReason::UnhandledType),
    ));

    let response = router
        .oneshot(
            Request::post("/webhooks/stripe")
                .header("Stripe-Signature", "t=1,v1=fake")
                .body(Body::from("{}"))?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn duplicate_receipt_returns_200_without_calling_the_handler() -> Result<(), Box<dyn Error>> {
    let verifier = Arc::new(StubVerifier::new(VerifierResponse::Duplicate));
    // A handler that would fail loudly if it were ever reached: `Duplicate`
    // has no `VerifiedEvent` to hand it, so if this ran the route is wrong
    // in a way that would panic before this outcome is even relevant.
    let router = billing_router(app_state(
        verifier,
        EventOutcome::NotApplied(NotAppliedReason::UnhandledType),
    ));

    let response = router
        .oneshot(
            Request::post("/webhooks/stripe")
                .header("Stripe-Signature", "t=1,v1=fake")
                .body(Body::from("{}"))?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn verification_failure_returns_400_problem_json() -> Result<(), Box<dyn Error>> {
    let verifier = Arc::new(StubVerifier::new(VerifierResponse::VerificationFailed));
    let router = billing_router(app_state(
        verifier,
        EventOutcome::NotApplied(NotAppliedReason::UnhandledType),
    ));

    let response = router
        .oneshot(
            Request::post("/webhooks/stripe")
                .header("Stripe-Signature", "t=1,v1=wrong")
                .body(Body::from("{}"))?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    Ok(())
}

#[tokio::test]
async fn missing_signature_header_returns_400_no_panic() -> Result<(), Box<dyn Error>> {
    let verifier = Arc::new(StubVerifier::new(VerifierResponse::Fresh(verified_event())));
    let router = billing_router(app_state(
        verifier,
        EventOutcome::NotApplied(NotAppliedReason::UnhandledType),
    ));

    let response = router
        .oneshot(Request::post("/webhooks/stripe").body(Body::from("{}"))?)
        .await?;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    Ok(())
}

#[tokio::test]
async fn exact_request_bytes_reach_the_verifier_unmodified() -> Result<(), Box<dyn Error>> {
    // Deliberately not what a re-serialised `Value` would produce: unusual
    // whitespace and a key order a `serde_json::Value` round-trip would not
    // preserve. If anything between the socket and the verifier parses and
    // re-encodes the body, this exact byte sequence would not survive.
    let raw_payload = b"{\"b\": 2,   \"a\": 1}".to_vec();
    let verifier = Arc::new(StubVerifier::new(VerifierResponse::Fresh(verified_event())));
    let router = billing_router(app_state(
        verifier.clone(),
        EventOutcome::NotApplied(NotAppliedReason::UnhandledType),
    ));

    let response = router
        .oneshot(
            Request::post("/webhooks/stripe")
                .header("Stripe-Signature", "t=1,v1=fake")
                .body(Body::from(raw_payload.clone()))?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(verifier.captured_payload(), Some(raw_payload));
    Ok(())
}

#[tokio::test]
async fn not_applied_outcome_still_returns_200() -> Result<(), Box<dyn Error>> {
    let verifier = Arc::new(StubVerifier::new(VerifierResponse::Fresh(verified_event())));
    let router = billing_router(app_state(
        verifier,
        EventOutcome::NotApplied(NotAppliedReason::UnknownCustomer),
    ));

    let response = router
        .oneshot(
            Request::post("/webhooks/stripe")
                .header("Stripe-Signature", "t=1,v1=fake")
                .body(Body::from("{}"))?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}
