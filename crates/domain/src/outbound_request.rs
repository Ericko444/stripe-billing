use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DomainError, TenantId};

/// Identifies an `OutboundRequest`. Distinct from other entities' ids so the
/// compiler rejects passing the wrong id where an outbound request id is
/// expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutboundRequestId(Uuid);

impl OutboundRequestId {
    /// Wraps a raw `Uuid` as an `OutboundRequestId`.
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the underlying `Uuid`.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

/// A record that a mutating Stripe call was about to be made under a given
/// idempotency key.
///
/// Append-only ledger: no soft delete, no `list`. Unlike `WebhookEvent`,
/// `tenant_id` is always known — an outbound call originates from a tenant
/// context. The *reuse* logic that reads this back (and sets `completed_at`
/// / `stripe_object_id`) is Phase 2's job; Phase 1 only records the attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundRequest {
    /// The request's id.
    pub id: OutboundRequestId,
    /// The tenant the call originates from.
    pub tenant_id: TenantId,
    /// The logical operation (e.g. `create_subscription`).
    pub operation: String,
    /// A fingerprint of the request inputs, to detect a mismatched reuse.
    pub request_fingerprint: String,
    /// The idempotency key sent to Stripe. Globally unique.
    pub idempotency_key: String,
    /// The Stripe object id the call produced, once known.
    pub stripe_object_id: Option<String>,
    /// When the attempt was recorded.
    pub created_at: OffsetDateTime,
    /// When the call completed, if it has.
    pub completed_at: Option<OffsetDateTime>,
}

/// Port for persisting and looking up `OutboundRequest` records. Implemented
/// by an adapter crate (`persistence`); no I/O here.
///
/// Ledger-shaped: `create` plus a single lookup by idempotency key. No
/// `list`, no `find` by internal id, no mark-complete — those belong to the
/// Phase 2 reuse logic.
#[allow(async_fn_in_trait)]
pub trait OutboundRequestRepository {
    /// Records an outbound attempt. Fails if `idempotency_key` is already
    /// present (the global unique index) — surfaced as a `DomainError`, not a
    /// panic.
    async fn create(
        &self,
        tenant_id: TenantId,
        operation: String,
        request_fingerprint: String,
        idempotency_key: String,
    ) -> Result<OutboundRequest, DomainError>;

    /// Finds a recorded attempt by its idempotency key. Returns `None` if
    /// none has been recorded under that key.
    async fn find_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<OutboundRequest>, DomainError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_as_uuid() {
        let id = Uuid::new_v4();
        let request_id = OutboundRequestId::new(id);
        assert_eq!(request_id.as_uuid(), id);
    }
}
