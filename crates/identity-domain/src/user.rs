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
