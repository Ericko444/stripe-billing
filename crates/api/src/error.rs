use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use domain::DomainError;
use serde::Serialize;
use uuid::Uuid;

use crate::correlation::CorrelationId;

/// Errors this crate's route handlers can produce, mapped to
/// `application/problem+json` (RFC 9457) by `IntoResponse` below.
///
/// Three variants: a `domain` failure, the one error this crate itself can
/// produce before ever reaching `domain` -- a request missing a header a
/// route requires -- and `Unauthorized`, for a host's tenant extractor to
/// reject with. All three map to a status; none leaks internals to the
/// caller.
///
/// `Domain` carries an `Option<CorrelationId>` -- `None` via `?`'s blanket
/// `From<DomainError>` below, matching this crate's original
/// self-minting behaviour exactly; `Some` when a write route has attached
/// the id it read from `correlation::layer`,
/// via `ApiError::with_correlation_id`. `MissingSignatureHeader` and
/// `Unauthorized` never carry one: the first is the (tenant-less) webhook
/// route, the second is constructed by a host's own tenant extractor
/// *before* this crate's business logic -- and therefore before any
/// per-request id -- ever runs.
#[derive(Debug)]
pub enum ApiError {
    /// A failure surfaced by `domain` or a use case built on it.
    Domain(DomainError, Option<CorrelationId>),
    /// The `Stripe-Signature` header was absent from the request.
    MissingSignatureHeader,
    /// A host's [`TenantExtractor`](crate::TenantExtractor) could not
    /// authenticate the request. The one variant this crate defines for a
    /// host to reject with rather than derive from `DomainError` -- there is
    /// no domain concept of "not authenticated", only ports and tenants.
    Unauthorized,
}

impl From<DomainError> for ApiError {
    fn from(err: DomainError) -> Self {
        ApiError::Domain(err, None)
    }
}

impl ApiError {
    /// Attaches this request's correlation id, so the id in this error's
    /// response body and server-side log line is the one a successful call
    /// on the same request would have carried into its audit entry --
    /// rather than a fresh id minted only because this call happened to
    /// fail. A no-op on `MissingSignatureHeader` and `Unauthorized`: see
    /// this type's own docs for why those two never carry one.
    pub(crate) fn with_correlation_id(self, id: CorrelationId) -> Self {
        match self {
            ApiError::Domain(err, _) => ApiError::Domain(err, Some(id)),
            other => other,
        }
    }
}

/// RFC 9457 problem details body. `type` is always `"about:blank"` -- no
/// route in this crate yet defines its own problem type URIs, and
/// `about:blank` is the RFC's own placeholder for exactly that case.
#[derive(Debug, Serialize)]
struct ProblemDetails {
    #[serde(rename = "type")]
    type_uri: &'static str,
    title: &'static str,
    status: u16,
    detail: &'static str,
    correlation_id: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // The request's own id when a write route attached one
        // (`with_correlation_id`); a fresh mint otherwise -- the same
        // self-sufficient fallback this crate always had, now also covering
        // `MissingSignatureHeader`, `Unauthorized`, and any `Domain` error
        // that reached here via `?`'s blanket `From` rather than a route
        // that read the request's id. Never accepted from an inbound
        // header, in either case: this route is public and unauthenticated
        // (webhooks carry no token, and a host's tenant extractor runs
        // before any handler body does), so trusting a caller-supplied id
        // would let an attacker plant whatever they want in our own logs.
        let correlation_id = match &self {
            ApiError::Domain(_, Some(id)) => id.as_uuid(),
            _ => Uuid::new_v4(),
        };

        let (status, title, detail) = match &self {
            ApiError::Domain(DomainError::WebhookVerification, _) => (
                StatusCode::BAD_REQUEST,
                "Webhook verification failed",
                "The request signature could not be verified.",
            ),
            ApiError::MissingSignatureHeader => (
                StatusCode::BAD_REQUEST,
                "Missing signature header",
                "The Stripe-Signature header was not present on the request.",
            ),
            // A host's tenant extractor rejected the request. One `detail`
            // shared by every cause -- no token, a malformed header, a
            // bad signature and an expired token are all the same response,
            // so the body itself cannot be used to probe which is which.
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "Not authenticated",
                "The request could not be authenticated.",
            ),
            // A client-supplied value that did not parse -- a pagination
            // cursor, in practice. The caller's mistake, so 400, not the
            // 500 the default arm would give it. `detail` stays coarse: the
            // specific parse failure is in `MalformedRequest`'s string and
            // goes to the log line below, not the response.
            ApiError::Domain(DomainError::MalformedRequest(_), _) => (
                StatusCode::BAD_REQUEST,
                "Malformed request",
                "A parameter in the request could not be parsed.",
            ),
            // Used by `GET /invoices/{id}` for an unknown id **and** for
            // another tenant's id -- the repository's tenant-scoped `find`
            // returns `None` for both, so the two are one code path and one
            // response. A 403 for "exists but not yours" would let the status
            // code alone confirm another tenant holds that id.
            ApiError::Domain(DomainError::NotFound, _) => (
                StatusCode::NOT_FOUND,
                "Not found",
                "The requested resource was not found.",
            ),
            // A write lost a race for a record it needed -- in practice the
            // idempotency ledger refusing to guess the outcome of a prior
            // in-flight attempt. 409, not the 500 the
            // default arm would give it, so the caller is told to retry the
            // logical operation rather than left thinking the request was
            // malformed. `detail` carries nothing caller-specific.
            ApiError::Domain(DomainError::Conflict, _) => (
                StatusCode::CONFLICT,
                "Conflict",
                "The request conflicts with the current state of the resource.",
            ),
            // A `BillingProvider` call failed -- Stripe returned an error, or
            // was unreachable. 502: the failure is upstream of us, not the
            // caller's request. The provider's own message is in
            // `Provider(String)` and reaches the log line below; it never
            // reaches the caller (a provider error can carry account or
            // schema internals).
            ApiError::Domain(DomainError::Provider(_), _) => (
                StatusCode::BAD_GATEWAY,
                "Upstream provider error",
                "An upstream provider failed to process the request.",
            ),
            // Every other DomainError -- Repository, MalformedEvent, and any
            // variant added later -- falls to 500. This is
            // deliberately the default arm, not an enumerated list: a new
            // DomainError variant must not silently acquire a 4xx because a
            // wildcard elsewhere guessed at it.
            ApiError::Domain(_, _) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal error",
                "An internal error occurred while processing the request.",
            ),
        };

        // The full error goes server-side, keyed by the same id the caller
        // receives -- `detail` above is deliberately coarse (a raw
        // DomainError rendered into a response body can expose schema or
        // provider internals), so this is the only place the real cause is
        // recorded.
        tracing::error!(
            error = %DisplayError(&self),
            correlation_id = %correlation_id,
            "request failed"
        );

        let body = ProblemDetails {
            type_uri: "about:blank",
            title,
            status: status.as_u16(),
            detail,
            correlation_id: correlation_id.to_string(),
        };

        let mut response = (status, Json(body)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        if matches!(self, ApiError::Unauthorized) {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

/// Renders the real error for the server-side log line. `ApiError` itself
/// has no `Display` -- nothing outside this module should be tempted to
/// put it in front of a caller -- so this is private and used only above.
struct DisplayError<'a>(&'a ApiError);

impl std::fmt::Display for DisplayError<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            ApiError::Domain(err, _) => write!(f, "{err}"),
            ApiError::MissingSignatureHeader => write!(f, "missing Stripe-Signature header"),
            ApiError::Unauthorized => write!(f, "request not authenticated"),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    async fn response_json(err: ApiError) -> (StatusCode, serde_json::Value) {
        let response = err.into_response();
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        assert_eq!(content_type.as_deref(), Some("application/problem+json"));

        let bytes = to_bytes(response.into_body(), usize::MAX).await;
        let body: serde_json::Value = bytes
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    #[tokio::test]
    async fn webhook_verification_maps_to_400() {
        let (status, body) =
            response_json(ApiError::Domain(DomainError::WebhookVerification, None)).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["status"], 400);
    }

    #[tokio::test]
    async fn missing_signature_header_maps_to_400() {
        let (status, _body) = response_json(ApiError::MissingSignatureHeader).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unmapped_domain_error_falls_to_500() {
        // `Conflict` and `Provider` now have their own arms, so the
        // default-arm test must use a variant that still falls through --
        // `MalformedEvent` is one that has no mapping of its own.
        let (status, body) = response_json(ApiError::Domain(
            DomainError::MalformedEvent("shape".into()),
            None,
        ))
        .await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["status"], 500);
    }

    #[tokio::test]
    async fn conflict_maps_to_409() {
        let (status, body) = response_json(ApiError::Domain(DomainError::Conflict, None)).await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["status"], 409);
    }

    #[tokio::test]
    async fn provider_error_maps_to_502_and_reveals_no_internals() {
        let (status, body) = response_json(ApiError::Domain(
            DomainError::Provider(
                "No such customer: 'cus_123'; a similar object exists in test mode".to_string(),
            ),
            None,
        ))
        .await;

        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["status"], 502);
        // The provider's own message must not reach the caller.
        let rendered = body.to_string();
        assert!(!rendered.contains("cus_123"));
        assert!(!rendered.contains("test mode"));
    }

    #[tokio::test]
    async fn not_found_maps_to_404() {
        let (status, body) = response_json(ApiError::Domain(DomainError::NotFound, None)).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["status"], 404);
    }

    #[tokio::test]
    async fn malformed_request_maps_to_400() {
        let (status, body) = response_json(ApiError::Domain(
            DomainError::MalformedRequest("bad cursor".into()),
            None,
        ))
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["status"], 400);
        // The specific parse reason must not reach the caller.
        assert!(!body.to_string().contains("bad cursor"));
    }

    #[tokio::test]
    async fn repository_error_body_reveals_no_internals() {
        let (_, body) = response_json(ApiError::Domain(
            DomainError::Repository(
                "duplicate key value violates unique constraint \"customers_pkey\" on table billing.customers"
                    .to_string(),
            ),
            None,
        ))
        .await;

        let rendered = body.to_string();
        assert!(!rendered.contains("customers_pkey"));
        assert!(!rendered.contains("billing.customers"));
        assert!(!rendered.contains("constraint"));
    }

    #[tokio::test]
    async fn unauthorized_maps_to_401_with_www_authenticate_bearer() {
        let response = ApiError::Unauthorized.into_response();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer")
        );
    }

    #[tokio::test]
    async fn correlation_id_is_present_and_looks_like_a_uuid() {
        let (_, body) = response_json(ApiError::Domain(DomainError::Conflict, None)).await;

        let correlation_id = body["correlation_id"].as_str().unwrap_or_default();
        assert!(Uuid::parse_str(correlation_id).is_ok());
    }
}
