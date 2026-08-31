//! Axum router factory, wire DTOs and the error-to-HTTP mapping.
//!
//! Exposes a `Router`, never a binary and never a `main` -- a host that
//! already owns `tokio::main` and its own config loading mounts this
//! crate's router into its own server (`init-spec.md` §4). See `error.rs`
//! for the RFC 9457 mapping and `state.rs` for what a host constructs
//! before mounting the router.

mod error;
mod state;

pub use error::ApiError;
pub use state::AppState;
