use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

/// This request's correlation id.
///
/// Minted once by [`layer`] and inserted into the request's extensions --
/// never accepted from an inbound header. A write route reads it out
/// (`Extension<CorrelationId>`) and attaches it to an [`ApiError`](crate::ApiError)
/// on failure, so the id in that response's body is the same one a
/// successful call on the same request would have carried forward. A route
/// that does not read it (every read route, and any route mounted without
/// [`layer`]) is unaffected: `ApiError` falls back to minting its own, the
/// same behaviour this crate had before this type existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrelationId(Uuid);

impl CorrelationId {
    /// Wraps a raw `Uuid` as a `CorrelationId`.
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the underlying `Uuid`.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

/// Middleware that mints one [`CorrelationId`] per request and inserts it
/// into the request's extensions before the handler runs.
///
/// Deliberately does not read any inbound header (`X-Correlation-Id` or
/// otherwise): this route is reachable by an unauthenticated caller until a
/// host's tenant extractor has run, and trusting a caller-supplied id here
/// would let an attacker plant whatever they want in our own logs and audit
/// trail -- the same reasoning `ApiError`'s own id generation has always
/// used, just moved one layer out and applied before the handler runs
/// rather than only after it fails.
pub async fn layer(mut request: Request, next: Next) -> Response {
    request
        .extensions_mut()
        .insert(CorrelationId::new(Uuid::new_v4()));
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_as_uuid() {
        let id = Uuid::new_v4();
        let correlation_id = CorrelationId::new(id);
        assert_eq!(correlation_id.as_uuid(), id);
    }
}
