use time::OffsetDateTime;

use crate::{Action, Actor, CorrelationId, Target, TenantId};

/// One past fact. Constructed, written, and never touched again.
///
/// Every field is a typed id or a closed enum. There is deliberately
/// **no** free-form payload -- no `String`, no `HashMap<String, String>`,
/// no `details: Option<String>` "for debugging" -- so a secret, a token or
/// a password has nowhere here to go. That is the property this crate was
/// asked to make structural rather than reviewed for: it is checkable by
/// reading this struct's field list, not by auditing every call site that
/// constructs one.
///
/// Fields are private. A `pub` field is a field a caller can assign
/// directly, which would let a careless caller build an entry the
/// constructor would otherwise have refused -- a correlation id
/// copy-pasted from a different request, for instance. The single
/// constructor is where this crate can still enforce that, now or later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    tenant_id: TenantId,
    actor: Actor,
    action: Action,
    target: Target,
    occurred_at: OffsetDateTime,
    correlation_id: CorrelationId,
}

impl AuditEntry {
    /// Records one fact.
    ///
    /// `occurred_at` is taken from the caller rather than stamped here
    /// with `OffsetDateTime::now_utc()`: a caller writing inside a
    /// transaction should record the time the operation began, not the
    /// time this constructor happens to run.
    pub fn new(
        tenant_id: TenantId,
        actor: Actor,
        action: Action,
        target: Target,
        occurred_at: OffsetDateTime,
        correlation_id: CorrelationId,
    ) -> Self {
        Self {
            tenant_id,
            actor,
            action,
            target,
            occurred_at,
            correlation_id,
        }
    }

    /// The tenant the action was performed within.
    pub fn tenant_id(&self) -> TenantId {
        self.tenant_id
    }

    /// Who performed the action.
    pub fn actor(&self) -> Actor {
        self.actor
    }

    /// What happened.
    pub fn action(&self) -> Action {
        self.action
    }

    /// What it happened to.
    pub fn target(&self) -> Target {
        self.target
    }

    /// When it happened.
    pub fn occurred_at(&self) -> OffsetDateTime {
        self.occurred_at
    }

    /// The request this entry was written as part of.
    pub fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::{SubjectId, TargetId};

    #[test]
    fn accessors_return_exactly_what_new_was_given() {
        let tenant_id = TenantId::new(Uuid::new_v4());
        let actor = Actor::User(SubjectId::new(Uuid::new_v4()));
        let action = Action::PaymentMethodDetached;
        let target = Target::PaymentMethod(TargetId::new(Uuid::new_v4()));
        let occurred_at = OffsetDateTime::now_utc();
        let correlation_id = CorrelationId::new(Uuid::new_v4());

        let entry = AuditEntry::new(
            tenant_id,
            actor,
            action,
            target,
            occurred_at,
            correlation_id,
        );

        assert_eq!(entry.tenant_id(), tenant_id);
        assert_eq!(entry.actor(), actor);
        assert_eq!(entry.action(), action);
        assert_eq!(entry.target(), target);
        assert_eq!(entry.occurred_at(), occurred_at);
        assert_eq!(entry.correlation_id(), correlation_id);
    }
}
