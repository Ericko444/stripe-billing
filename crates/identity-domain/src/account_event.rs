use audit::{Action, Actor, CorrelationId, Target};
use time::OffsetDateTime;

/// Something that happened to an **account** rather than within one tenant
/// -- a password reset, a password change, a profile update.
///
/// A user belongs to several tenants and `audit_log.tenant_id` is required,
/// so an account event is recorded once **per tenant the user is an active
/// member of**, all rows sharing one correlation id. That is the decision
/// taken over a nullable, platform-scoped row: every tenant's own audit view
/// shows its member's security events, frozen at the moment they happened,
/// without a join on today's memberships rewriting yesterday's history.
///
/// This type is everything an entry needs except the tenant; the adapter
/// supplies each tenant inside the business transaction, from the
/// memberships true at commit. The two costs, named: one fact is N rows
/// (count distinct correlation ids, not rows), and a user with no active
/// membership gets no row at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountEvent {
    /// Who acted.
    pub actor: Actor,
    /// What happened.
    pub action: Action,
    /// What it happened to -- normally `Target::User`.
    pub target: Target,
    /// When it happened.
    pub occurred_at: OffsetDateTime,
    /// The request it happened in.
    pub correlation_id: CorrelationId,
}
