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

/// What deactivating an account did.
///
/// `BelongsToOtherTenants` exists so the repository can refuse a request it
/// alone can adjudicate -- whether the caller's tenant is the account's only
/// one is a fact about rows, decided under the same lock as the write, not
/// something a use case can check first without racing an invitation to a
/// second tenant.
///
/// **The API must not distinguish it from an ordinary refusal.** Telling an
/// Admin of one tenant that the target also belongs to another discloses a
/// membership of a tenant they have no part in -- the one thing the module's
/// isolation exists to prevent. This variant is for the use case's decision
/// and for tests; over HTTP it and "you may not" are one `403`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeactivateOutcome {
    /// The account is deactivated now, and was not before. Its sessions and
    /// any outstanding reset or invitation link are gone.
    Deactivated,
    /// The account was already deactivated. Nothing was written and nothing
    /// was recorded: the state asked for is the state it is in -- the same
    /// contract as suspending an already-suspended membership.
    AlreadyDeactivated,
    /// The caller's tenant is not the account's only active membership, so
    /// this caller may not end it everywhere. Nothing was written. The
    /// tenant-scoped tool for this is suspension.
    BelongsToOtherTenants,
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

/// Longest display name accepted, in Unicode scalar values.
pub const MAX_DISPLAY_NAME_CHARS: usize = 100;

/// A display name as a user set it: trimmed, at most
/// [`MAX_DISPLAY_NAME_CHARS`], no control characters. May be empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayName(String);

impl DisplayName {
    /// Validates and trims `raw`.
    pub fn parse(raw: &str) -> Result<Self, DisplayNameError> {
        let trimmed = raw.trim();
        if trimmed.chars().count() > MAX_DISPLAY_NAME_CHARS || trimmed.chars().any(char::is_control)
        {
            return Err(DisplayNameError);
        }
        Ok(Self(trimmed.to_string()))
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A display name was too long or contained control characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid display name")]
pub struct DisplayNameError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_are_trimmed_bounded_and_printable() {
        assert_eq!(
            DisplayName::parse("  Alice  ").map(|n| n.as_str().to_string()),
            Ok("Alice".to_string())
        );
        assert!(DisplayName::parse("").is_ok());
        assert!(DisplayName::parse(&"é".repeat(MAX_DISPLAY_NAME_CHARS)).is_ok());
        assert_eq!(
            DisplayName::parse(&"é".repeat(MAX_DISPLAY_NAME_CHARS + 1)),
            Err(DisplayNameError)
        );
        assert_eq!(DisplayName::parse("Ali\u{0}ce"), Err(DisplayNameError));
        assert_eq!(DisplayName::parse("Ali\nce"), Err(DisplayNameError));
    }
}
