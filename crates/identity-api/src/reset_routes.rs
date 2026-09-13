use audit::CorrelationId;
use axum::Json;
use axum::extract::Extension;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use identity_domain::Email;
use identity_service::ResetJob;

use crate::client_ip::ClientAddress;
use crate::dto::{RESET_REQUEST_ACCEPTED, ResetRequest};
use crate::error::{ErrorKind, IdentityError};
use crate::limits;
use crate::state::IdentityState;

/// `POST /auth/password-reset/request`.
///
/// **This handler never learns whether the address has an account**, and so
/// cannot answer differently -- in body or in time -- depending on it. It
/// works through [`IdentityState::reset_requests`], which holds the rate
/// limiter and the reset queue and nothing else: it parses the address,
/// counts the request, puts the address on the queue, and answers `202` with
/// the one fixed body. Looking the address up, issuing a token, writing the
/// audit rows and sending the mail all happen afterwards, in the reset
/// worker.
///
/// Answers other than `202`, none of which depends on the address having an
/// account: `400` for a body or address that does not parse, `429` when the
/// client IP is over its limit. An address over *its* limit still gets `202`
/// -- dropped without saying so.
pub async fn request_password_reset(
    Extension(state): Extension<IdentityState>,
    Extension(correlation_id): Extension<CorrelationId>,
    ClientAddress(ip): ClientAddress,
    body: Result<Json<ResetRequest>, JsonRejection>,
) -> Result<Response, IdentityError> {
    let requests = state.reset_requests();
    let error = |kind| IdentityError::new(kind, correlation_id);

    limits::reset_request_by_ip(requests.limiter, ip, correlation_id)?;
    let Json(request) =
        body.map_err(|rejection| error(ErrorKind::MalformedRequest(rejection.body_text())))?;
    let email = Email::parse(&request.email)
        .map_err(|_| error(ErrorKind::MalformedRequest("invalid email address".into())))?;

    let uuid = correlation_id.as_uuid();
    if !limits::reset_request_by_address(requests.limiter, &email) {
        tracing::info!(correlation_id = %uuid, "reset request over the per-address limit; dropped");
    } else if !requests.queue.enqueue(ResetJob {
        email,
        correlation_id,
    }) {
        tracing::warn!(correlation_id = %uuid, "reset queue full or stopped; request dropped");
    }

    let mut response = (StatusCode::ACCEPTED, Json(RESET_REQUEST_ACCEPTED)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}
