use uuid::Uuid;

/// Identifies a tenant. Distinct from other entities' ids so the compiler
/// rejects passing the wrong id where a tenant id is expected.
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
