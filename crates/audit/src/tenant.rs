use uuid::Uuid;

/// Identifies a tenant, local to this crate.
///
/// A parallel type to a host module's own tenant id, not a re-export of
/// one -- `audit` must not depend on `domain` or any other workspace crate
/// (that dependency list is the checkable proof of its neutrality), so
/// each module converts its own tenant id to this one at the boundary
/// where it constructs an [`AuditEntry`](crate::AuditEntry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TenantId(Uuid);

impl TenantId {
    /// Wraps a raw `Uuid` as a `TenantId`.
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the underlying `Uuid`.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_as_uuid() {
        let id = Uuid::new_v4();
        let tenant_id = TenantId::new(id);
        assert_eq!(tenant_id.as_uuid(), id);
    }
}
