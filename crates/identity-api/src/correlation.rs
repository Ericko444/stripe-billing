use audit::CorrelationId;
use axum::extract::Request;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

/// Middleware minting one [`CorrelationId`] per request into the request
/// extensions, before any extractor or handler runs.
///
/// The billing module's rule, extended rather than redone: the id is minted
/// server-side and **never** read from an inbound header. Routes here are
/// reachable unauthenticated -- login and password reset exist for callers
/// with no session -- so trusting a caller-supplied id would let anyone plant
/// a chosen value in our logs and audit rows. `audit::CorrelationId` is used
/// directly: this crate already depends on `audit`, and a third newtype over
/// the same `Uuid` would only add a conversion.
pub async fn layer(mut request: Request, next: Next) -> Response {
    request
        .extensions_mut()
        .insert(CorrelationId::new(Uuid::new_v4()));
    next.run(request).await
}

/// This request's correlation id, as minted by [`layer`] -- or a fresh one
/// if the router was mounted without it, so an error response always has an
/// id to report.
pub fn correlation_id(parts: &Parts) -> CorrelationId {
    parts
        .extensions
        .get::<CorrelationId>()
        .copied()
        .unwrap_or_else(|| CorrelationId::new(Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::Extension;
    use axum::http::Request;
    use axum::middleware;
    use axum::routing::get;
    use tower::ServiceExt;

    use super::*;

    async fn echo(Extension(id): Extension<CorrelationId>) -> String {
        id.as_uuid().to_string()
    }

    fn router() -> Router {
        Router::new()
            .route("/", get(echo))
            .layer(middleware::from_fn(layer))
    }

    async fn body_of(request: Request<Body>) -> Result<String, Box<dyn Error>> {
        let response = router().oneshot(request).await?;
        let bytes = to_bytes(response.into_body(), usize::MAX).await?;
        Ok(String::from_utf8(bytes.to_vec())?)
    }

    #[tokio::test]
    async fn an_inbound_correlation_header_is_ignored() -> Result<(), Box<dyn Error>> {
        let spoofed = "11111111-1111-1111-1111-111111111111";
        let body = body_of(
            Request::builder()
                .uri("/")
                .header("x-correlation-id", spoofed)
                .body(Body::empty())?,
        )
        .await?;

        assert!(Uuid::parse_str(&body).is_ok());
        assert_ne!(body, spoofed);
        Ok(())
    }

    #[tokio::test]
    async fn each_request_gets_its_own_id() -> Result<(), Box<dyn Error>> {
        let first = body_of(Request::builder().uri("/").body(Body::empty())?).await?;
        let second = body_of(Request::builder().uri("/").body(Body::empty())?).await?;
        assert_ne!(first, second);
        Ok(())
    }
}
