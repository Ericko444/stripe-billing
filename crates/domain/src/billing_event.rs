use async_trait::async_trait;
use thiserror::Error;

use crate::{SubscriptionId, TenantId};

/// A typed notification handed to the host after a webhook event has been
/// mirrored (`init-spec.md` §8.3).
///
/// A domain enum, not a re-exported Stripe type: a variant carrying a Stripe
/// object, or a raw payload `Value`, would couple every host implementation
/// to Stripe's wire format, and the abstraction `BillingEventSink` exists for
/// would be fictional. A host must be able to implement
/// [`BillingEventSink`] without adding a Stripe client to its own manifest.
///
/// Starts with the three variants `customer.subscription.updated` produces;
/// later event types (`invoice.payment_failed`'s `PaymentFailed`, and so on)
/// add variants here as their handlers land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BillingEvent {
    /// The subscription became (or remained) active -- a new subscriber, or
    /// one recovering from a past-due or incomplete state.
    SubscriptionActivated {
        /// The tenant the subscription belongs to.
        tenant_id: TenantId,
        /// The local subscription mirror row.
        subscription_id: SubscriptionId,
    },
    /// The subscription ended, whether canceled by the tenant or after
    /// failed payment retries.
    SubscriptionCanceled {
        /// The tenant the subscription belongs to.
        tenant_id: TenantId,
        /// The local subscription mirror row.
        subscription_id: SubscriptionId,
    },
    /// The subscription changed in some way not captured by `Activated` or
    /// `Canceled` -- for example a period rollover or a change to
    /// `cancel_at_period_end` with no status change.
    SubscriptionUpdated {
        /// The tenant the subscription belongs to.
        tenant_id: TenantId,
        /// The local subscription mirror row.
        subscription_id: SubscriptionId,
    },
}

/// The host's error type for a failed [`BillingEventSink::handle`] call.
///
/// Deliberately a single opaque variant: by the time this reaches `service`
/// (a later phase), the mirror write has already succeeded and does not roll
/// back (§8.3) -- the failure is logged against the correlation id, not
/// interpreted. The host is free to wrap whatever it wants in the message.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("billing event sink failed: {0}")]
pub struct SinkError(pub String);

/// Port through which the module notifies its host of a mirrored change.
/// Implemented by the host; no I/O here.
///
/// `#[async_trait]` and `Send + Sync` for the same reason
/// [`BillingProvider`](crate::BillingProvider) and
/// [`WebhookVerifier`](crate::WebhookVerifier) have them: a later phase's
/// `api` holds this as a `dyn BillingEventSink` in shared application state,
/// wired at runtime to whatever the host provides.
///
/// Two behaviours are fixed at the port, not left to the implementation
/// (§8.3): deduplication and mirror persistence happen *before* `handle` is
/// called, so the sink never sees an event that is not already recorded and
/// already reflected in the mirror tables; and a sink failure must not roll
/// back the mirror -- the mirror is a fact about what Stripe said, the sink
/// is a side effect of it.
#[async_trait]
pub trait BillingEventSink: Send + Sync {
    /// Notifies the host of a mirrored billing event.
    async fn handle(&self, event: BillingEvent) -> Result<(), SinkError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compiles only if `BillingEventSink` is dyn-compatible -- the property
    /// the `#[async_trait]` decision exists for.
    #[allow(dead_code)]
    fn assert_dyn_compatible(_sink: &dyn BillingEventSink) {}
}
