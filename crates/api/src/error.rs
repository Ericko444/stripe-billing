use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use domain::DomainError;
use serde::Serialize;
use uuid::Uuid;

/// Errors this crate's route handlers can produce, mapped to
/// `application/problem+json` (RFC 9457) by `IntoResponse` below.
///
/// Two variants only: a `domain` failure, and the one error this crate
/// itself can produce before ever reaching `domain` -- a request missing a
/// header a route requires. Both map to a status; neither leaks internals
/// to the caller (`init-spec.md` §5.5).
#[derive(Debug)]
pub enum ApiError {
    /// A failure surfaced by `domain` or a use case built on it.
    Domain(DomainError),
    /// The `Stripe-Signature` header was absent from the request.
    MissingSignatureHeader,
}

impl From<DomainError> for ApiError {
    fn from(err: DomainError) -> Self {
        ApiError::Domain(err)
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
        // Generated here, per request, and used for both the body and the
        // log line below -- never accepted from an inbound header. This
        // route is public and unauthenticated (webhooks carry no token), so
        // trusting a caller-supplied id would let an attacker plant
        // whatever they want in our own logs.
        let correlation_id = Uuid::new_v4();

        let (status, title, detail) = match &self {
            ApiError::Domain(DomainError::WebhookVerification) => (
                StatusCode::BAD_REQUEST,
                "Webhook verification failed",
                "The request signature could not be verified.",
            ),
            ApiError::MissingSignatureHeader => (
                StatusCode::BAD_REQUEST,
                "Missing signature header",
                "The Stripe-Signature header was not present on the request.",
            ),
            // A client-supplied value that did not parse -- a pagination
            // cursor, in practice. The caller's mistake, so 400, not the
            // 500 the default arm would give it. `detail` stays coarse: the
            // specific parse failure is in `MalformedRequest`'s string and
            // goes to the log line below, not the response.
            ApiError::Domain(DomainError::MalformedRequest(_)) => (
                StatusCode::BAD_REQUEST,
                "Malformed request",
                "A parameter in the request could not be parsed.",
            ),
            // Used by `GET /invoices/{id}` for an unknown id **and** for
            // another tenant's id -- the repository's tenant-scoped `find`
            // returns `None` for both, so the two are one code path and one
            // response. A 403 for "exists but not yours" would let the status
            // code alone confirm another tenant holds that id.
            ApiError::Domain(DomainError::NotFound) => (
                StatusCode::NOT_FOUND,
                "Not found",
                "The requested resource was not found.",
            ),
            // Every other DomainError -- Repository, Provider, Conflict,
            // MalformedEvent, and any variant a later phase adds -- falls to
            // 500. This is deliberately the default arm, not an enumerated
            // list: a new DomainError variant must not silently acquire a 4xx
            // because a wildcard elsewhere guessed at it.
            ApiError::Domain(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal error",
                "An internal error occurred while processing the request.",
            ),
        };

        // The full error goes server-side, keyed by the same id the caller
        // receives -- `detail` above is deliberately coarse (§5.5: a raw
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
            ApiError::Domain(err) => write!(f, "{err}"),
            ApiError::MissingSignatureHeader => write!(f, "missing Stripe-Signature header"),
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
            response_json(ApiError::Domain(DomainError::WebhookVerification)).await;

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
        let (status, body) = response_json(ApiError::Domain(DomainError::Conflict)).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["status"], 500);
    }

    #[tokio::test]
    async fn not_found_maps_to_404() {
        let (status, body) = response_json(ApiError::Domain(DomainError::NotFound)).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["status"], 404);
    }

    #[tokio::test]
    async fn malformed_request_maps_to_400() {
        let (status, body) = response_json(ApiError::Domain(DomainError::MalformedRequest(
            "bad cursor".into(),
        )))
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["status"], 400);
        // The specific parse reason must not reach the caller.
        assert!(!body.to_string().contains("bad cursor"));
    }

    #[tokio::test]
    async fn repository_error_body_reveals_no_internals() {
        let (_, body) = response_json(ApiError::Domain(DomainError::Repository(
            "duplicate key value violates unique constraint \"customers_pkey\" on table billing.customers"
                .to_string(),
        )))
        .await;

        let rendered = body.to_string();
        assert!(!rendered.contains("customers_pkey"));
        assert!(!rendered.contains("billing.customers"));
        assert!(!rendered.contains("constraint"));
    }

    #[tokio::test]
    async fn correlation_id_is_present_and_looks_like_a_uuid() {
        let (_, body) = response_json(ApiError::Domain(DomainError::Conflict)).await;

        let correlation_id = body["correlation_id"].as_str().unwrap_or_default();
        assert!(Uuid::parse_str(correlation_id).is_ok());
    }
}
