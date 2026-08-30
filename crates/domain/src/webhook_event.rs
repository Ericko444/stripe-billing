use std::future::Future;

use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DomainError, TenantId};

/// Identifies a `WebhookEvent`. Distinct from other entities' ids so the
/// compiler rejects passing the wrong id where a webhook event id is expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WebhookEventId(Uuid);

impl WebhookEventId {
    /// Wraps a raw `Uuid` as a `WebhookEventId`.
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the underlying `Uuid`.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

/// A received Stripe webhook event, stored verbatim for audit and replay.
///
/// This is an append-only ledger: no soft delete, no tenant-scoped index,
/// no `list`. `tenant_id` is optional because the tenant is resolved *from*
/// the event by `service` code sometime after receipt (`init-spec.md` §10.3),
/// not known at insert time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookEvent {
    /// The event's id.
    pub id: WebhookEventId,
    /// The tenant this event belongs to, once resolved.
    pub tenant_id: Option<TenantId>,
    /// Stripe's own event id (`evt_...`). Globally unique — the dedup anchor.
    pub stripe_event_id: String,
    /// The Stripe event type (e.g. `invoice.paid`).
    pub event_type: String,
    /// The raw event body. Nothing parses it in Phase 1.
    pub payload: Value,
    /// When the event was received and stored.
    pub created_at: OffsetDateTime,
    /// When the event finished processing, if it has. Phase 3 records events
    /// with this NULL and dedups on `stripe_event_id` alone; setting it once
    /// downstream processing completes is Phase 4's job (`init-spec.md` §10.2).
    pub processed_at: Option<OffsetDateTime>,
}

/// Port for persisting and looking up `WebhookEvent` records. Implemented by
/// an adapter crate (`persistence`); no I/O here.
///
/// Ledger-shaped: `create` plus a single lookup by Stripe's event id (the
/// dedup anchor). No `list`, no `find` by internal id — nothing needs them.
///
/// Like `OutboundRequestRepository`, this port's methods are written as
/// `fn … -> impl Future<Output = …> + Send` rather than bare `async fn`.
/// `stripe-adapter`'s `StripeWebhookVerifier<R>` implements the
/// `#[async_trait]` `WebhookVerifier` port by awaiting these methods;
/// `#[async_trait]` boxes its futures as `Send`, so the futures it awaits
/// must be `Send` too — which a bare `async fn` in a trait does not promise
/// for a generic `R`. Implementors may still write `async fn` in the `impl`
/// block; the bound is checked there.
pub trait WebhookEventRepository {
    /// Stores a received event. Fails if `stripe_event_id` is already present
    /// (the global unique index) — surfaced as a `DomainError`, not a panic.
    fn create(
        &self,
        tenant_id: Option<TenantId>,
        stripe_event_id: String,
        event_type: String,
        payload: Value,
    ) -> impl Future<Output = Result<WebhookEvent, DomainError>> + Send;

    /// Finds a stored event by Stripe's event id. Returns `None` if none has
    /// been stored under that id.
    fn find_by_stripe_event_id(
        &self,
        stripe_event_id: &str,
    ) -> impl Future<Output = Result<Option<WebhookEvent>, DomainError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_as_uuid() {
        let id = Uuid::new_v4();
        let event_id = WebhookEventId::new(id);
        assert_eq!(event_id.as_uuid(), id);
    }
}
