use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use identity_domain::{Role, TenantId, UserId};
use identity_service::{ActiveSession, SessionError};

use crate::cookie::session_token;
use crate::correlation::correlation_id;
use crate::error::{ErrorKind, IdentityError};
use crate::state::IdentityState;

/// The caller's session, accepted: an Axum extractor over **any** router
/// state.
///
/// Generic over the state on purpose. A host mounting another module's
/// router -- billing's, whose state type this crate must not name -- wraps
/// this extractor in a ten-line newtype of its own and gets an authenticated
/// session there too. The session service travels in a request extension
/// ([`IdentityState`]), not in router state, for the same reason.
///
/// Every failure short of an outage is the same `401`: no cookie, a
/// malformed token, an unknown or forged one, an expired session, a
/// deactivated user, a suspended membership.
#[derive(Debug, Clone)]
pub struct AuthenticatedSession(ActiveSession);

impl AuthenticatedSession {
    /// The session as the service accepted it.
    pub fn session(&self) -> &ActiveSession {
        &self.0
    }

    /// Who the caller is.
    pub fn user_id(&self) -> UserId {
        self.0.user_id
    }

    /// The tenant the session is scoped to, if one has been picked.
    pub fn tenant_id(&self) -> Option<TenantId> {
        self.0.tenant.map(|tenant| tenant.tenant_id)
    }

    /// The caller's role in that tenant, read from the membership on this
    /// request.
    pub fn role(&self) -> Option<Role> {
        self.0.tenant.map(|tenant| tenant.role)
    }
}

impl<S> FromRequestParts<S> for AuthenticatedSession
where
    S: Send + Sync,
{
    type Rejection = IdentityError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let correlation_id = correlation_id(parts);

        // A missing extension is a wiring mistake in the host, not a caller
        // error -- a 500 makes it loud instead of disguising it as "logged
        // out".
        let state = parts
            .extensions
            .get::<IdentityState>()
            .cloned()
            .ok_or_else(|| {
                IdentityError::new(
                    ErrorKind::Internal("IdentityState extension is not installed".into()),
                    correlation_id,
                )
            })?;

        let Some(token) = session_token(&parts.headers).map(str::to_owned) else {
            return Err(IdentityError::new(ErrorKind::Unauthorized, correlation_id));
        };

        match state.authentication().authenticate(&token).await {
            Ok(session) => Ok(Self(session)),
            Err(SessionError::Unauthenticated) => {
                Err(IdentityError::new(ErrorKind::Unauthorized, correlation_id))
            }
            Err(SessionError::Unavailable(reason)) => Err(IdentityError::new(
                ErrorKind::Internal(reason),
                correlation_id,
            )),
        }
    }
}
