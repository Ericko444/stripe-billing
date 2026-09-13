use uuid::Uuid;

/// Identifies one request end to end.
///
/// Minted exactly once per request, by the host's own middleware -- never
/// accepted from an inbound header, and never re-minted by this crate.
/// `audit` only carries the id it is handed; it has no opinion on how a
/// caller obtains one, which is what keeps this crate free of an HTTP
/// dependency of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CorrelationId(Uuid);

impl CorrelationId {
    /// Wraps a raw `Uuid` as a `CorrelationId`.
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
        let correlation_id = CorrelationId::new(id);
        assert_eq!(correlation_id.as_uuid(), id);
    }
}
