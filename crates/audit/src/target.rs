use uuid::Uuid;

/// Identifies the target of an action -- opaque to this crate, the same
/// reasoning as [`crate::SubjectId`]: a target's real id type belongs to
/// whichever module the target lives in (`domain::PaymentMethodId`, for
/// example), and `audit` must not depend on that module to name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TargetId(Uuid);

impl TargetId {
    /// Wraps a raw `Uuid` as a `TargetId`.
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the underlying `Uuid`.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

/// What the action was performed on -- a closed enum that grows one
/// variant at a time, for the same reason [`Action`](crate::Action) does:
/// each module's vertical slice adds the variant it needs, as it lands.
///
/// A variant added here carries a typed [`TargetId`] -- never a raw
/// string, and never another module's id type re-exported (the same
/// reasoning as [`crate::TenantId`] and [`crate::SubjectId`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// A payment method.
    PaymentMethod(TargetId),
    /// A subscription.
    Subscription(TargetId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_id_round_trips_through_as_uuid() {
        let id = Uuid::new_v4();
        let target_id = TargetId::new(id);
        assert_eq!(target_id.as_uuid(), id);
    }
}
