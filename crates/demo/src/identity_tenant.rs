//! The seam between the two modules: the identity module's session extractor,
//! wrapped so it satisfies the billing module's `TenantExtractor`.
//!
//! **This file is the proof that billing's boundary was right.** Billing's
//! `api` crate defines `TenantExtractor` and has never heard of the identity
//! module; the identity module's `identity-api` defines
//! `AuthenticatedSession` and has never heard of billing. Neither crate
//! changed to make them meet -- the host writes these few lines, in the one
//! crate allowed to name both.
//!
//! It has to live here, not in `identity-api`: `TenantExtractor` requires
//! `FromRequestParts<api::AppState, Rejection = api::ApiError>`, and naming
//! either type would make the identity module depend on billing.
//!
//! The claim is checkable: `git diff --stat` over `crates/api` (and
//! `domain`, `service`, `persistence`, `stripe-adapter`) across the whole
//! identity phase prints nothing.
//!
//! # What holding the boundary costs
//!
//! Billing's contract with a host is one value: a `TenantId`. Keeping that
//! contract unchanged is the point, and it has three prices, accepted rather
//! than overlooked:
//!
//! - **No role-based billing authorization.** Any active membership may use
//!   billing's routes -- a Member can cancel the tenant's subscription. A role
//!   check would need billing to receive a role, which is the `api` change
//!   this seam exists to avoid.
//! - **No `403` from billing.** This newtype cannot see which route it is
//!   extracting for, and billing's `ApiError` has one refusal, `401`.
//!   Refusing a Member here would answer a valid session with "not
//!   authenticated" -- which the frontend reads as logged out -- and would
//!   block reads along with writes.
//! - **Billing's audit rows still say `Actor::System`.** Its request context
//!   carries a tenant and a correlation id, not a user; naming the user would
//!   again mean widening billing's contract.
//!
//! Each is a follow-up that starts with a deliberate change to `api`, not a
//! workaround in the host.

use api::{ApiError, AppState};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use domain::TenantId;
use identity_api::AuthenticatedSession;

/// The tenant an identity session is scoped to, as billing's `TenantId`.
/// Satisfies `TenantExtractor` by blanket impl.
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
        // Every failure -- no cookie, a refused or expired session, a
        // suspended membership, a deactivated user, a session with no tenant
        // picked yet, a missing extension -- becomes billing's one
        // `Unauthorized`. The response must not say which.
        let session = AuthenticatedSession::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::Unauthorized)?;
        let tenant = session.tenant_id().ok_or(ApiError::Unauthorized)?;

        // The two modules' tenant ids are distinct types over the same uuid;
        // this is the one place both are named.
        Ok(IdentityTenant(TenantId::new(tenant.as_uuid())))
    }
}
