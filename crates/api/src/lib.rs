//! Axum router factory, wire DTOs and the error-to-HTTP mapping.
//!
//! Three rules this crate carries:
//!
//! - **Exposes a `Router`, never a binary and never a `main`** -- a host
//!   that already owns `tokio::main` and its own config loading mounts this
//!   crate's router into its own server (`init-spec.md` §4).
//! - **The webhook body is read as raw `Bytes`, never `Json<T>`** -- the
//!   Stripe signature is an HMAC over the exact bytes sent, and a `Json<T>`
//!   extractor consuming and re-encoding the body breaks verification
//!   irrecoverably (§10.1). `Bytes` is the last extractor so `HeaderMap` is
//!   available first.
//! - **DTOs are `api`-owned; domain types are never `Serialize`.** Every
//!   response body is a type in `dto.rs` with a hand-written `From` impl. The
//!   moment a domain struct is serializable, adding a field to it becomes a
//!   wire-breaking change made by someone not thinking about the wire (§9).
//!   Money is `{amount_minor, currency}` -- never a float, never a
//!   preformatted string (S5); timestamps are RFC 3339 UTC.
//!
//! # Two factories, on purpose
//!
//! [`webhook_router`] mounts `POST /webhooks/stripe` and nothing else. It is
//! **not** generic: that route authenticates with the Stripe signature and
//! has no tenant (`init-spec.md` §10.3). [`billing_router`] mounts every
//! tenant-scoped route and *is* generic over a host-supplied tenant
//! extractor `T` (§8.1 Option B). Splitting them keeps a tenant extractor
//! from ever becoming a precondition of the webhook route. A host serves one,
//! the other, or both merged.
//!
//! # Wiring a host (read this before Phase 4c / a real deployment)
//!
//! The contract a host implements is [`TenantExtractor`]: an Axum
//! `FromRequestParts` extractor that **rejects with [`ApiError`]** and
//! converts `Into<`[`TenantId`](domain::TenantId)`>`. Pinning the rejection
//! type is what keeps every error out of the router in
//! `application/problem+json`. Forgetting to supply a `T` is a compile
//! error, not a runtime 500. [`billing_router`]'s own rustdoc carries a
//! complete, compiling example of the ~15 lines a middleware-based host
//! writes.
//!
//! See `error.rs` for the RFC 9457 mapping (where information leaks are
//! prevented), `extract.rs` for the `T` bound, `dto.rs` for the wire types,
//! and `state.rs` for what a host constructs before mounting.

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post};

mod dto;
mod error;
mod extract;
mod routes;
mod state;

pub use dto::{
    CancelRequest, ChangePlanRequest, InvoiceDto, InvoicePageDto, MoneyDto, PageParams,
    PaymentMethodDto, PlanDto, SetupIntentDto, SubscriptionDto,
};
pub use error::ApiError;
pub use extract::TenantExtractor;
pub use state::AppState;

/// Bytes limit for the webhook body. Nothing in `WebhookVerifier` bounds the
/// input it will HMAC -- Phase 3 documented that as a host precondition on
/// the port's own rustdoc, and this is where the host (this route) enforces
/// it. Stripe's own events are small (typically well under 64 KiB); this
/// leaves generous headroom without leaving the limit effectively unbounded.
const WEBHOOK_BODY_LIMIT_BYTES: usize = 256 * 1024;

/// Builds the router for `POST /webhooks/stripe`, and nothing else.
///
/// Not generic, deliberately. This is the one route in the module with no
/// tenant: it authenticates with the `Stripe-Signature` header, not with
/// anything a tenant extractor would produce (`init-spec.md` §10.3). Mounting
/// it here rather than on [`billing_router`] means making that router generic
/// can never turn a tenant extractor into a precondition of the webhook
/// route -- a regression that would surface as Stripe getting 401s on a route
/// whose auth is its signature.
///
/// A host serves this alone (as `demo` does until the tenant-scoped routes
/// are wired), or `.merge()`s it into [`billing_router`]'s output.
pub fn webhook_router(state: AppState) -> Router {
    Router::new()
        .route(
            "/webhooks/stripe",
            post(routes::webhooks::post_stripe_webhook),
        )
        .layer(DefaultBodyLimit::max(WEBHOOK_BODY_LIMIT_BYTES))
        .with_state(state)
}

/// Builds the billing module's tenant-scoped router, generic over the host's
/// tenant extractor `T`.
///
/// A factory function returning a `Router`, not a binary: this crate never
/// owns `main`, `tokio::main` or config loading, so a host that already has
/// them can mount this router into its own server (`init-spec.md` §4).
///
/// `T` is the host's own type. It is bound by [`TenantExtractor`] -- an Axum
/// [`FromRequestParts`](axum::extract::FromRequestParts) extractor that
/// **rejects with [`ApiError`]** and converts [`Into`] a
/// [`TenantId`](domain::TenantId). Pinning the rejection type is what keeps
/// every error out of this router in `application/problem+json`. Failing to
/// supply a `T` is a compile error, not a runtime 500.
///
/// This crate never reads a tenant from a path, query, header or body: the
/// only way a `TenantId` enters a handler is out of `T` (§8.1 -- accepting
/// `tenant_id` as a request parameter is textbook IDOR).
///
/// # Adapting a middleware-based host ("Option A")
///
/// A host whose auth layer already put the tenant in the request extensions
/// writes a newtype and names it as `T`. This compiles as written:
///
/// ```
/// use api::{ApiError, AppState, billing_router};
/// use axum::extract::FromRequestParts;
/// use axum::http::request::Parts;
/// use domain::{DomainError, TenantId};
///
/// // The host's adapter: pull the `TenantId` its middleware inserted.
/// struct HostTenant(TenantId);
///
/// impl FromRequestParts<AppState> for HostTenant {
///     type Rejection = ApiError;
///
///     async fn from_request_parts(
///         parts: &mut Parts,
///         _state: &AppState,
///     ) -> Result<Self, Self::Rejection> {
///         parts
///             .extensions
///             .get::<TenantId>()
///             .copied()
///             .map(HostTenant)
///             // any 4xx `ApiError` the host prefers for "not authenticated"
///             .ok_or(ApiError::from(DomainError::NotFound))
///     }
/// }
///
/// impl From<HostTenant> for TenantId {
///     fn from(tenant: HostTenant) -> Self {
///         tenant.0
///     }
/// }
///
/// // The host then mounts the router with its own type as `T`:
/// fn mount(state: AppState) -> axum::Router {
///     billing_router::<HostTenant>(state)
/// }
/// # let _ = mount;
/// ```
pub fn billing_router<T>(state: AppState) -> Router
where
    T: TenantExtractor,
{
    // Each route names `T` as its tenant extractor. That is the only place a
    // `TenantId` enters a handler -- never a path, query, header or body.
    Router::new()
        .route("/plans", get(routes::plans::list_plans::<T>))
        .route(
            "/subscription",
            get(routes::subscription::get_subscription::<T>),
        )
        .route(
            "/subscriptions/{id}/change-plan",
            post(routes::subscriptions::change_plan::<T>),
        )
        .route(
            "/subscriptions/{id}/cancel",
            post(routes::subscriptions::cancel_subscription::<T>),
        )
        .route(
            "/payment-methods",
            get(routes::payment_methods::list_payment_methods::<T>),
        )
        .route(
            "/payment-methods/setup-intent",
            post(routes::payment_methods::create_setup_intent::<T>),
        )
        .route(
            "/payment-methods/{id}/default",
            post(routes::payment_methods::set_default_payment_method::<T>),
        )
        .route(
            "/payment-methods/{id}",
            delete(routes::payment_methods::remove_payment_method::<T>),
        )
        .route("/invoices", get(routes::invoices::list_invoices::<T>))
        .route("/invoices/{id}", get(routes::invoices::get_invoice::<T>))
        .with_state(state)
}
