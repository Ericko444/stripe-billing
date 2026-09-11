use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use domain::WebhookReceipt;

use crate::{ApiError, AppState};

/// `POST /webhooks/stripe`. Verifies the request against the
/// `Stripe-Signature` header, dedups it, and -- for a first delivery --
/// hands the verified event to the configured `WebhookHandler`.
///
/// `body: Bytes`, never `Json<T>`, and it is the **last** extractor: the
/// signature is an HMAC over the exact bytes Stripe sent, so `HeaderMap` is
/// read first and nothing touches the body before
/// `webhook_verifier.verify_and_record` sees it untouched. A `Json<T>`
/// extractor would consume and re-encode the body first, breaking
/// verification irrecoverably.
///
/// Every code path that reaches the handler or stops at `Duplicate` returns
/// **200** -- a business outcome, never an error (`Fresh` handled,
/// `Duplicate`, and every `EventOutcome` all acknowledge; anything else
/// would only make Stripe retry). Only a
/// missing header or a verification failure is a 400; anything else is a
/// genuine fault and falls through `?` to the default 500.
pub(crate) async fn post_stripe_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let signature_header = headers
        .get("Stripe-Signature")
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::MissingSignatureHeader)?;

    let receipt = state
        .webhook_verifier
        .verify_and_record(&body, signature_header)
        .await?;

    let event = match receipt {
        WebhookReceipt::Fresh(event) => event,
        // Acknowledged, not reprocessed -- the type itself carries no
        // VerifiedEvent to reprocess even if this arm wanted to.
        WebhookReceipt::Duplicate { .. } => return Ok(StatusCode::OK),
    };

    // Every EventOutcome the handler can return -- Applied or NotApplied,
    // for any reason -- is a 200. The handler already decided nothing
    // further needs doing; the route has no more say in it.
    state.webhook_handler.handle(event).await?;

    Ok(StatusCode::OK)
}
