//! The identity module's HTTP surface: a router factory, the session cookie,
//! the request correlation id, and the `application/problem+json` error
//! mapping.
//!
//! Like the billing module's `api`, this crate exposes a `Router`, never a
//! binary: a host mounts it beside whatever else it serves. Unlike `api`, it
//! authenticates -- that is the module's job -- and it names no billing
//! crate. The host (`demo`) is the one place the two meet.

mod cookie;
mod correlation;
mod error;

pub use cookie::{SESSION_COOKIE, clearing_cookie, session_cookie, session_token};
pub use correlation::{correlation_id, layer as correlation_layer};
pub use error::{ErrorKind, IdentityError};
