use std::str::FromStr;

use audit::CorrelationId;
use thiserror::Error;
use time::OffsetDateTime;

use crate::{Email, MembershipId, Role, Selector, TenantId, UserId, VerifierHash};

/// Whether a membership grants access to its tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MembershipStatus {
    /// Grants access.
    Active,
    /// Kept for the record, grants nothing: the session lookup refuses a
    /// session scoped to it.
    Suspended,
}

impl MembershipStatus {
    /// The stored and wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            MembershipStatus::Active => "active",
            MembershipStatus::Suspended => "suspended",
        }
    }
}

impl FromStr for MembershipStatus {
    type Err = MembershipStatusParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(MembershipStatus::Active),
            "suspended" => Ok(MembershipStatus::Suspended),
            _ => Err(MembershipStatusParseError),
        }
    }
}

/// A string did not name a membership status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("unknown membership status")]
pub struct MembershipStatusParseError;

/// One member of a tenant, as the tenant's Owners and Admins see it.
///
/// Deliberately without the display name, or whether the account has a
/// password yet: both would tell an inviting tenant whether the address
/// already had an account somewhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantMember {
    /// The membership's id.
    pub membership_id: MembershipId,
    /// The member.
    pub user_id: UserId,
    /// The member's address.
    pub email: Email,
    /// The member's role in this tenant.
    pub role: Role,
    /// Active or suspended.
    pub status: MembershipStatus,
}

/// The invitation link to store if the added address turns out to have no
/// password -- selector and verifier hash, never the verifier, like every
/// stored token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInvitation {
    /// Finds the row.
    pub selector: Selector,
    /// `SHA-256(verifier)`.
    pub verifier_hash: VerifierHash,
    /// When it stops working.
    pub expires_at: OffsetDateTime,
}

/// Everything adding an address to a tenant might write, decided before the
/// transaction that learns whether the address has an account.
///
/// The invitation is minted up front for both outcomes and simply not used
/// when the account already has a password -- so the use case does the same
/// work, in the same order, either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberGrant {
    /// The tenant the address is added to.
    pub tenant_id: TenantId,
    /// The address added.
    pub email: Email,
    /// The role granted.
    pub role: Role,
    /// The Owner or Admin adding it -- the audit actor.
    pub granted_by: UserId,
    /// Stored if the account has no password, new or not.
    pub invitation: PendingInvitation,
    /// When, by the module's clock -- the audit rows' and the token's
    /// `created_at`.
    pub occurred_at: OffsetDateTime,
    /// The request doing it.
    pub correlation_id: CorrelationId,
}

/// What adding an address to a tenant did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantOutcome {
    /// The membership exists now.
    Granted {
        /// The new member.
        member: TenantMember,
        /// Whether the invitation was stored: the account has no password,
        /// so the mail to send carries a link to set one. Never shown to the
        /// caller.
        invitation_issued: bool,
    },
    /// The account already has a membership in this tenant, active or
    /// suspended. Nothing was written. Safe to say: the tenant can list its
    /// own members.
    AlreadyMember,
}
