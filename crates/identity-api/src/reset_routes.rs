use audit::CorrelationId;
use axum::Json;
use axum::extract::Extension;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use identity_domain::{Email, Password};
use identity_service::{CompleteResetError, ResetJob};

use crate::client_ip::ClientAddress;
use crate::dto::{CompleteResetRequest, RESET_REQUEST_ACCEPTED, ResetRequest};
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
/// This is the timing defence, and it is structural rather than simulated:
/// there is no dummy lookup, no artificial sleep, no attempt to make two
/// code paths cost the same. The request path simply contains no work whose
/// cost depends on the account, so there is nothing for a timer to measure.
/// (Login, which must answer whether a password is right, takes the other
/// approach: an unknown address is verified against a dummy Argon2id hash.)
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

/// `POST /auth/password-reset/complete`.
///
/// `204` and nothing else on success -- in particular **no `Set-Cookie`**:
/// completing a reset ends every session the user had, and does not start a
/// new one. `422` for a password the policy refuses (the link stays usable),
/// one `400` for every link that cannot be used, `429` past the per-IP limit.
///
/// Every answer, success or not, carries `Referrer-Policy: no-referrer` and
/// `Cache-Control: no-store`: the page that sends this request has the token
/// in its URL fragment, and nothing it loads should be told where it came
/// from.
pub async fn complete_password_reset(
    Extension(state): Extension<IdentityState>,
    Extension(correlation_id): Extension<CorrelationId>,
    ClientAddress(ip): ClientAddress,
    body: Result<Json<CompleteResetRequest>, JsonRejection>,
) -> Response {
    let mut response = match complete(&state, correlation_id, ip, body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error.into_response(),
    };
    let headers = response.headers_mut();
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn complete(
    state: &IdentityState,
    correlation_id: CorrelationId,
    ip: identity_domain::ClientIp,
    body: Result<Json<CompleteResetRequest>, JsonRejection>,
) -> Result<(), IdentityError> {
    let error = |kind| IdentityError::new(kind, correlation_id);

    limits::reset_complete_by_ip(state.limiter(), ip, correlation_id)?;
    let Json(request) =
        body.map_err(|rejection| error(ErrorKind::MalformedRequest(rejection.body_text())))?;
    let new_password = Password::new(request.new_password);

    state
        .password_resets()
        .complete_reset(&request.token, new_password, correlation_id)
        .await
        .map_err(|err| match err {
            CompleteResetError::Policy(_) => error(ErrorKind::PasswordPolicy),
            CompleteResetError::InvalidLink => error(ErrorKind::InvalidResetLink),
            CompleteResetError::Unavailable(reason) => error(ErrorKind::Internal(reason)),
        })
}
