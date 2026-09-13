use std::str::FromStr;

use thiserror::Error;

/// What a user may do within one tenant. One role per membership, not per
/// user: the same person can own one tenant and be a member of another.
///
/// A closed enum, for the reason `audit`'s `Action` is one: a role a caller
/// could name freely is a role nothing can reason about exhaustively.
/// Granting and suspension rules live with the members use cases, which
/// match on this enum and are forced to decide every variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Full control of the tenant, including its other Owners.
    Owner,
    /// Manages members, but cannot grant or suspend an Owner.
    Admin,
    /// Uses the tenant; manages nothing.
    Member,
}

impl Role {
    /// The stored and wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }
}

impl FromStr for Role {
    type Err = RoleParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "owner" => Ok(Role::Owner),
            "admin" => Ok(Role::Admin),
            "member" => Ok(Role::Member),
            _ => Err(RoleParseError),
        }
    }
}

/// A string did not name a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("unknown role")]
pub struct RoleParseError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_its_spelling() {
        for role in [Role::Owner, Role::Admin, Role::Member] {
            assert_eq!(role.as_str().parse::<Role>(), Ok(role));
        }
    }

    #[test]
    fn spelling_is_exact() {
        assert_eq!("Owner".parse::<Role>(), Err(RoleParseError));
        assert_eq!("".parse::<Role>(), Err(RoleParseError));
    }
}
