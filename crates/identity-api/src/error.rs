use audit::CorrelationId;
use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// An identity route's failure, carrying the request's correlation id so the
/// id in the response body is the id in the server log line -- and, on a
/// write that succeeded partway, the id on its audit rows.
///
/// Unlike the billing module's `ApiError::Unauthorized`, which mints a fresh
/// id because a host's extractor runs before that crate can read one, every
/// error here is built where the request's parts are in reach, including in
/// the session extractor's rejection.
#[derive(Debug)]
pub struct IdentityError {
    kind: ErrorKind,
    correlation_id: CorrelationId,
}

/// What went wrong. Each kind maps to one status and one fixed `detail`; the
/// variable part, where there is one, goes to the log line only.
#[derive(Debug)]
pub enum ErrorKind {
    /// No valid session, or credentials that did not verify. **One kind for
    /// both**, and one body: an unknown address, a wrong password, an expired
    /// session and a missing cookie are indistinguishable to the caller.
    Unauthorized,
    /// The named resource does not exist **or is not the caller's** -- one
    /// kind for both, so the status does not confirm that an id is real.
    NotFound,
    /// A request body or parameter did not parse. The reason is logged.
    MalformedRequest(String),
    /// Something this module depends on failed. The reason is logged.
    Internal(String),
}

impl IdentityError {
    /// An error of `kind` for the request identified by `correlation_id`.
    pub fn new(kind: ErrorKind, correlation_id: CorrelationId) -> Self {
        Self {
            kind,
            correlation_id,
        }
    }

    /// What went wrong.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }
}

/// RFC 9457 problem details -- the same five members as the billing module's
/// `api` crate (`crates/api/src/error.rs`, `ProblemDetails`), so the frontend
/// parses both modules' errors with one function. This crate may not depend
/// on `api` to share the type; `the_body_has_exactly_apis_members` pins the
/// shape instead.
#[derive(Debug, Serialize)]
struct ProblemDetails {
    #[serde(rename = "type")]
    type_uri: &'static str,
    title: &'static str,
    status: u16,
    detail: &'static str,
    correlation_id: String,
}

impl IntoResponse for IdentityError {
    fn into_response(self) -> Response {
        let (status, title, detail) = match &self.kind {
            ErrorKind::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "Not authenticated",
                "The request could not be authenticated.",
            ),
            ErrorKind::NotFound => (
                StatusCode::NOT_FOUND,
                "Not found",
                "The requested resource was not found.",
            ),
            ErrorKind::MalformedRequest(_) => (
                StatusCode::BAD_REQUEST,
                "Malformed request",
                "The request could not be parsed.",
            ),
            ErrorKind::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal error",
                "An internal error occurred while processing the request.",
            ),
        };

        let reason = match &self.kind {
            ErrorKind::MalformedRequest(reason) | ErrorKind::Internal(reason) => reason.as_str(),
            ErrorKind::Unauthorized => "not authenticated",
            ErrorKind::NotFound => "not found",
        };
        let correlation_id = self.correlation_id.as_uuid();
        if status.is_server_error() {
            tracing::error!(%correlation_id, reason, "identity request failed");
        } else {
            tracing::info!(%correlation_id, reason, status = status.as_u16(), "identity request refused");
        }

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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error;

    use axum::body::to_bytes;
    use uuid::Uuid;

    use super::*;

    async fn render(kind: ErrorKind) -> Result<(StatusCode, serde_json::Value), Box<dyn Error>> {
        let correlation_id = CorrelationId::new(Uuid::new_v4());
        let response = IdentityError::new(kind, correlation_id).into_response();
        let status = response.status();
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/problem+json")
        );
        let bytes = to_bytes(response.into_body(), usize::MAX).await?;
        let body: serde_json::Value = serde_json::from_slice(&bytes)?;
        assert_eq!(
            body["correlation_id"].as_str(),
            Some(correlation_id.as_uuid().to_string().as_str())
        );
        Ok((status, body))
    }

    #[tokio::test]
    async fn the_body_has_exactly_apis_members() -> Result<(), Box<dyn Error>> {
        let (_, body) = render(ErrorKind::Unauthorized).await?;
        let members: BTreeSet<&str> = body
            .as_object()
            .map(|object| object.keys().map(String::as_str).collect())
            .unwrap_or_default();

        // The billing module's `ProblemDetails`, member for member.
        let apis: BTreeSet<&str> = ["type", "title", "status", "detail", "correlation_id"]
            .into_iter()
            .collect();
        assert_eq!(members, apis);
        Ok(())
    }

    #[tokio::test]
    async fn statuses_map_as_documented() -> Result<(), Box<dyn Error>> {
        assert_eq!(
            render(ErrorKind::Unauthorized).await?.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(render(ErrorKind::NotFound).await?.0, StatusCode::NOT_FOUND);
        assert_eq!(
            render(ErrorKind::MalformedRequest("x".into())).await?.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            render(ErrorKind::Internal("x".into())).await?.0,
            StatusCode::INTERNAL_SERVER_ERROR
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_logged_reason_never_reaches_the_body() -> Result<(), Box<dyn Error>> {
        let (_, body) = render(ErrorKind::Internal(
            "relation identity.sessions does not exist".into(),
        ))
        .await?;
        assert!(!body.to_string().contains("identity.sessions"));
        Ok(())
    }
}
