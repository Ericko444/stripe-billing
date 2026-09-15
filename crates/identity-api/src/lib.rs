//! The identity module's HTTP surface: a router factory, the session cookie,
//! the session extractor, the request correlation id, and the
//! `application/problem+json` error mapping.
//!
//! Like the billing module's `api`, this crate exposes a `Router`, never a
//! binary: a host mounts it beside whatever else it serves. Unlike `api`, it
//! authenticates -- that is the module's job -- and it names no billing
//! crate. The host (`demo`) is the one place the two meet: it wraps
//! [`AuthenticatedSession`] in a newtype satisfying billing's
//! `TenantExtractor`, and nothing in billing changes for it.

use axum::Router;
use axum::extract::{DefaultBodyLimit, Extension};
use axum::middleware;
use axum::routing::{get, post};

mod client_ip;
mod cookie;
mod correlation;
mod csrf;
mod dto;
mod error;
pub mod limits;
mod members_routes;
mod reset_routes;
mod routes;
mod session_extract;
mod state;

pub use client_ip::ClientAddress;
pub use cookie::{SESSION_COOKIE, clearing_cookie, session_cookie, session_token};
pub use correlation::{correlation_id, layer as correlation_layer};
pub use csrf::{AllowedOrigins, origin_check};
pub use error::{ErrorKind, IdentityError};
pub use session_extract::AuthenticatedSession;
pub use state::{IdentityState, ResetRequests};

/// Largest request body an identity route reads. Every body here is a
/// handful of short strings; the limit bounds what an unauthenticated caller
/// can make the server buffer and parse.
const BODY_LIMIT_BYTES: usize = 16 * 1024;

/// Builds the identity routes: `POST /auth/login`, `GET` and `PATCH /auth/me`,
/// `POST /auth/tenant`, `POST /auth/logout`, `POST /auth/password/change`,
/// the two password reset routes, `GET` and `POST /tenant/members`, and
/// `POST /tenant/members/{id}/suspend`.
///
/// Installs its own correlation layer and its own [`IdentityState`]
/// extension. A host that also wants [`AuthenticatedSession`] on *other*
/// routers installs the same state over those with `.layer(Extension(..))`.
pub fn identity_router(state: IdentityState) -> Router {
    Router::new()
        .route("/auth/login", post(routes::login))
        .route("/auth/me", get(routes::me).patch(routes::update_me))
        .route("/auth/tenant", post(routes::select_tenant))
        .route("/auth/logout", post(routes::logout))
        .route("/auth/password/change", post(routes::change_password))
        .route(
            "/auth/password-reset/request",
            post(reset_routes::request_password_reset),
        )
        .route(
            "/auth/password-reset/complete",
            post(reset_routes::complete_password_reset),
        )
        .route(
            "/tenant/members",
            get(members_routes::list_members).post(members_routes::add_member),
        )
        .route(
            "/tenant/members/{id}/suspend",
            post(members_routes::suspend_member),
        )
        .route(
            "/tenant/members/{id}/deactivate",
            post(members_routes::deactivate_member),
        )
        .route(
            "/tenant/members/{id}/reactivate",
            post(members_routes::reactivate_member),
        )
        .route("/auth/deactivate", post(members_routes::deactivate_self))
        .layer(DefaultBodyLimit::max(BODY_LIMIT_BYTES))
        .layer(Extension(state))
        .layer(middleware::from_fn(correlation::layer))
}
