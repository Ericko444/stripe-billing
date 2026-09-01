//! Axum router factory, wire DTOs and the error-to-HTTP mapping.
//!
//! Two rules this crate carries:
//!
//! - **Exposes a `Router`, never a binary and never a `main`** -- a host
//!   that already owns `tokio::main` and its own config loading mounts this
//!   crate's router into its own server (`init-spec.md` §4).
//! - **The webhook body is read as raw `Bytes`, never `Json<T>`** -- the
//!   Stripe signature is an HMAC over the exact bytes sent, and a `Json<T>`
//!   extractor consuming and re-encoding the body breaks verification
//!   irrecoverably (§10.1). `Bytes` is the last extractor so `HeaderMap` is
//!   available first.
//!
//! See `error.rs` for the RFC 9457 mapping (where information leaks are
//! prevented) and `state.rs` for what a host constructs before mounting.

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::post;

mod error;
mod routes;
mod state;

pub use error::ApiError;
pub use state::AppState;

/// Bytes limit for the webhook body. Nothing in `WebhookVerifier` bounds the
/// input it will HMAC -- Phase 3 documented that as a host precondition on
/// the port's own rustdoc, and this is where the host (this route) enforces
/// it. Stripe's own events are small (typically well under 64 KiB); this
/// leaves generous headroom without leaving the limit effectively unbounded.
const WEBHOOK_BODY_LIMIT_BYTES: usize = 256 * 1024;

/// Builds the billing module's router.
///
/// A factory function returning a `Router`, not a binary: this crate never
/// owns `main`, `tokio::main` or config loading, so a host that already has
/// them can mount this router into its own server (`init-spec.md` §4).
pub fn billing_router(state: AppState) -> Router {
    Router::new()
        .route(
            "/webhooks/stripe",
            post(routes::webhooks::post_stripe_webhook),
        )
        .layer(DefaultBodyLimit::max(WEBHOOK_BODY_LIMIT_BYTES))
        .with_state(state)
}
