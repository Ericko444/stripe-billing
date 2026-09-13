use time::OffsetDateTime;

use crate::{Selector, TenantId, UserId, VerifierHash};

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
