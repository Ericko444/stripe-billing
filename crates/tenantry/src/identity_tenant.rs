//! The seam between the two modules: the identity module's session extractor,
//! wrapped so it satisfies the billing module's `TenantExtractor`.
//!
//! Billing's `tenantry-api` defines `TenantExtractor` and has never heard of
//! the identity module; `tenantry-identity-api` defines `AuthenticatedSession`
//! and has never heard of billing. Neither changed to make them meet. This
//! type cannot live in either one: `TenantExtractor` requires
//! `FromRequestParts<tenantry_api::AppState, Rejection = tenantry_api::ApiError>`,
//! and naming those types from the identity side would make identity depend
//! on billing. This crate is the first published one allowed to name both,
//! so this is where it ships.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use tenantry_api::{ApiError, AppState};
use tenantry_domain::TenantId;
use tenantry_identity_api::AuthenticatedSession;

/// The tenant an identity session is scoped to, as billing's `TenantId`.
///
/// Satisfies billing's `TenantExtractor` by its blanket impl, so it is the
/// type parameter of `billing_router`:
///
/// ```
/// use tenantry::IdentityTenant;
/// use tenantry::billing::api::{AppState, billing_router};
///
/// fn billing_routes(state: AppState) -> axum::Router {
///     billing_router::<IdentityTenant>(state)
/// }
/// ```
///
/// The identity module's state must be installed over the billing routes as
/// an `Extension`, which is how the wrapped extractor finds its session
/// service; `examples/host.rs` shows the whole composition.
///
/// Every refusal -- no cookie, an expired or refused session, a suspended
/// membership, a deactivated user, a session with no tenant chosen yet --
/// becomes billing's one `401`, and the response does not say which.
///
/// # What the seam costs
///
/// Billing's contract with a host is one value, a `TenantId`. Keeping that
/// contract unchanged has three prices, accepted rather than overlooked:
///
/// - **No role-based billing authorization.** Any active membership may use
///   billing's routes: a Member can cancel the tenant's subscription. A role
///   check would need billing to receive a role.
/// - **No `403` from billing.** This type cannot see which route it extracts
///   for, and billing's `ApiError` has one refusal, `401`. Refusing a Member
///   here would answer a valid session with "not authenticated", and would
///   block reads along with writes.
/// - **Billing's audit rows record `Actor::System`.** Billing's request
///   context carries a tenant and a correlation id, not a user.
///
/// Each is a follow-up that starts with a deliberate change to billing's
/// contract, not a workaround in the host.
#[derive(Debug, Clone, Copy)]
pub struct IdentityTenant(TenantId);

impl From<IdentityTenant> for TenantId {
    fn from(tenant: IdentityTenant) -> Self {
        tenant.0
    }
}

impl FromRequestParts<AppState> for IdentityTenant {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Every failure -- including a missing extension -- becomes billing's
        // one `Unauthorized`. The response must not say which.
        let session = AuthenticatedSession::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::Unauthorized)?;
        let tenant = session.tenant_id().ok_or(ApiError::Unauthorized)?;

        // The two modules' tenant ids are distinct types over the same uuid;
        // this is the one place both are named.
        Ok(IdentityTenant(TenantId::new(tenant.as_uuid())))
    }
}
