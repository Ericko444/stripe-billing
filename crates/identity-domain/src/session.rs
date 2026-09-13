use time::OffsetDateTime;

use crate::{Role, Selector, SessionId, TenantId, UserId, VerifierHash};

/// A session to be stored. Holds the token's selector and the verifier's
/// hash -- never the verifier -- so nothing built from this type can put a
/// usable credential into the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSession {
    /// Finds the row.
    pub selector: Selector,
    /// `SHA-256(verifier)`.
    pub verifier_hash: VerifierHash,
    /// Whose session.
    pub user_id: UserId,
    /// The tenant it is scoped to, or `None` for the short-lived session
    /// that exists only to pick a tenant.
    pub tenant_id: Option<TenantId>,
    /// When the user proved a password. Carried across tenant switches, so
    /// switching never extends a session's absolute lifetime.
    pub authenticated_at: OffsetDateTime,
    /// When the session stops being accepted.
    pub expires_at: OffsetDateTime,
}

/// A stored session found by its selector, as the session lookup returns
/// it: only when the user is active and, for a tenant-scoped session, the
/// membership is active too. Whether the presented verifier matches, and
/// whether the session has expired, are still for the caller to check --
/// the first in constant time, the second against the one [`Clock`].
///
/// [`Clock`]: crate::Clock
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSession {
    /// The session row.
    pub id: SessionId,
    /// `SHA-256(verifier)`, to check the presented verifier against.
    pub verifier_hash: VerifierHash,
    /// Whose session.
    pub user_id: UserId,
    /// The tenant and the role the user holds in it, or `None` for a
    /// tenant-less session.
    pub tenant: Option<SessionTenant>,
    /// When the user proved a password.
    pub authenticated_at: OffsetDateTime,
    /// When the session stops being accepted.
    pub expires_at: OffsetDateTime,
}

/// The tenant a session is scoped to, with the role read from the membership
/// at lookup time -- so a role change is seen on the next request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionTenant {
    /// The tenant.
    pub tenant_id: TenantId,
    /// The user's role in it.
    pub role: Role,
}
