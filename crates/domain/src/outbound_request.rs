use std::future::Future;

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
/// context. `completed_at` and `stripe_object_id` are set once the call
/// succeeds, by `mark_complete`; a row with `completed_at: None` is either
/// in flight or abandoned.
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
/// Ledger-shaped: `create`, two lookups, and `mark_complete`. Still no
/// `list`, no `find` by internal id — nothing here reads the ledger for
/// display, only to decide whether an idempotency key can be reused.
///
/// Unlike the other repository ports, this one's methods are written as
/// `fn … -> impl Future<Output = …> + Send` rather than bare `async fn`.
/// `stripe-adapter`'s `StripeBillingProvider<R>` implements the
/// `#[async_trait]` `BillingProvider` port by delegating through a
/// `Ledger<R>` that awaits these methods; `#[async_trait]` boxes its
/// futures as `Send`, so the futures it awaits must be `Send` too — which a
/// bare `async fn` in a trait does not promise for a generic `R`. The other
/// ports are only ever held generically (never behind a `dyn`, never inside
/// a `Send`-boxed future), so they keep the lighter `async fn` form.
/// Implementors may still write `async fn` in the `impl` block; the bound is
/// checked there.
pub trait OutboundRequestRepository {
    /// Records an outbound attempt. Fails if `idempotency_key` is already
    /// present (the global unique index) — surfaced as a `DomainError`, not a
    /// panic.
    fn create(
        &self,
        tenant_id: TenantId,
        operation: String,
        request_fingerprint: String,
        idempotency_key: String,
    ) -> impl Future<Output = Result<OutboundRequest, DomainError>> + Send;

    /// Finds a recorded attempt by its idempotency key. Returns `None` if
    /// none has been recorded under that key.
    fn find_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> impl Future<Output = Result<Option<OutboundRequest>, DomainError>> + Send;

    /// Finds the most recent recorded attempt for a tenant, operation and
    /// input fingerprint. Returns `None` if none has been recorded. This is
    /// the lookup the idempotency ledger's reuse logic drives: a retry of
    /// the same logical operation with the same inputs finds its way back to
    /// the original attempt's key through this method, not through
    /// `find_by_idempotency_key` (the retry does not know the key).
    fn find_by_fingerprint(
        &self,
        tenant_id: TenantId,
        operation: &str,
        request_fingerprint: &str,
    ) -> impl Future<Output = Result<Option<OutboundRequest>, DomainError>> + Send;

    /// Marks a recorded attempt complete: sets `completed_at` and records
    /// the Stripe object id the call produced. Scoped to `tenant_id` like
    /// every other method, even though `id` alone already identifies the
    /// row — an attempt to mark another tenant's row complete is a bug, and
    /// this makes it one the query itself refuses rather than one that
    /// merely goes unnoticed.
    fn mark_complete(
        &self,
        tenant_id: TenantId,
        id: OutboundRequestId,
        stripe_object_id: String,
    ) -> impl Future<Output = Result<OutboundRequest, DomainError>> + Send;
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
