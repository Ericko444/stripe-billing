//! The bound [`billing_router`](crate::billing_router) places on the host's
//! tenant extractor.
//!
//! Ports-and-adapters, §8.1 Option B: this crate never authenticates. The
//! host does, and hands the router a type that turns an authenticated
//! request into a [`TenantId`]. The bound below is what the compiler checks
//! that type against, so "forgot to wire an extractor" is a build error
//! rather than a 500 in production.

use axum::extract::FromRequestParts;
use domain::TenantId;

use crate::{ApiError, AppState};

/// What every tenant-scoped route requires of the host-supplied extractor.
///
/// This is a *trait alias by blanket impl*: there is nothing to write an
/// `impl` block for. A type satisfies `TenantExtractor` automatically when
/// it is
///
/// - an Axum [`FromRequestParts`] extractor against [`AppState`] — so it
///   runs before the body is touched and can read headers, extensions and
///   [`AppState`];
/// - **rejecting with [`ApiError`]** — the load-bearing clause. Pinning the
///   associated `Rejection` type means an extraction failure leaves this
///   router as `application/problem+json` (RFC 9457), the same shape as
///   every other error. Without it a host could reject with a bare
///   `StatusCode` and one route family would answer errors differently from
///   the rest — a hole exactly where a reviewer looks for one;
/// - convertible into a [`TenantId`] with [`Into`] — the extractor's whole
///   job, done once at the handler boundary;
/// - `Send + Sync + 'static` — Axum's requirement for anything it stores in
///   a handler's future.
///
/// # Adapting a middleware-based host ("Option A")
///
/// A host whose auth layer already resolves the tenant and inserts it into
/// the request extensions does not need Option B's full extractor. It writes
/// a newtype — about ten lines — and names that as `billing_router`'s type
/// parameter. The snippet lives on [`billing_router`](crate::billing_router)'s
/// own rustdoc, next to the call it plugs into.
pub trait TenantExtractor:
    FromRequestParts<AppState, Rejection = ApiError> + Into<TenantId> + Send + Sync + 'static
{
}

impl<T> TenantExtractor for T where
    T: FromRequestParts<AppState, Rejection = ApiError> + Into<TenantId> + Send + Sync + 'static
{
}
