use time::OffsetDateTime;

use crate::{Email, MembershipId, PasswordHash, Role, TenantId, UserId};

/// A person. Global across tenants: one address, one account, however many
/// tenants it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// The user's id.
    pub id: UserId,
    /// The normalised address the user logs in with.
    pub email: Email,
    /// What the user is called in the UI. May be empty.
    pub display_name: String,
    /// The Argon2id hash, or `None` for an invited user who has not set a
    /// password yet -- and so cannot log in.
    pub password_hash: Option<PasswordHash>,
    /// When the account was deactivated, if it was.
    pub deactivated_at: Option<OffsetDateTime>,
}

impl User {
    /// Whether the account may authenticate at all.
    pub fn is_active(&self) -> bool {
        self.deactivated_at.is_none()
    }
}

/// An **active** membership, as login and tenant selection see it.
/// Suspended memberships are never returned where this type is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    /// The membership's id.
    pub id: MembershipId,
    /// The member.
    pub user_id: UserId,
    /// The tenant.
    pub tenant_id: TenantId,
    /// The tenant's display name, for a tenant picker.
    pub tenant_name: String,
    /// The member's role in this tenant.
    pub role: Role,
}
