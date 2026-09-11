use async_trait::async_trait;
use serde_json::Value;
use time::OffsetDateTime;

use crate::{DomainError, WebhookEventId};

/// A verified, deduplicated Stripe webhook event, in domain terms.
///
/// Deliberately "dumb": the raw `payload` verbatim plus the few envelope
/// fields the ledger needs, and no typed Stripe event. An unrecognised event
/// type is therefore not a special case — it takes the identical path to a
/// known one, so a new Stripe event type can never cause a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedEvent {
    /// The ledger row's id, so the webhook processor can mark that exact
    /// row processed.
    pub id: WebhookEventId,
    /// Stripe's own event id (`evt_…`) — the dedup anchor.
    pub stripe_event_id: String,
    /// The Stripe event type (e.g. `invoice.paid`).
    pub event_type: String,
    /// Stripe's own timestamp for the event. Carried through, never compared
    /// — the replay-window check runs against the signature header's `t`,
    /// not this.
    pub created: OffsetDateTime,
    /// The raw event body, exactly as received.
    pub payload: Value,
}

/// The outcome of [`WebhookVerifier::verify_and_record`].
///
/// `Duplicate` carries **no** [`VerifiedEvent`], by design: a caller
/// physically cannot obtain the parsed event from a duplicate delivery, so
/// "process a duplicate anyway" is not an expressible mistake. The shape of
/// the return type does the work a comment would otherwise have to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookReceipt {
    /// First delivery. The event is recorded; the caller should process it.
    Fresh(VerifiedEvent),
    /// Already recorded by an earlier delivery. Acknowledge with 200 and do
    /// not reprocess -- an error status would only make Stripe retry.
    Duplicate {
        /// Stripe's event id, echoed so the caller can log the skip.
        stripe_event_id: String,
    },
}

/// Port for verifying a raw Stripe webhook body against its signature header
/// and recording the event in the dedup ledger. Implemented by
/// `stripe-adapter`; no I/O here.
///
/// `#[async_trait]` and `: Send + Sync` for the same reason
/// [`BillingProvider`](crate::BillingProvider) has them: `api` holds this
/// as a `dyn WebhookVerifier` in shared application state,
/// wired at runtime, rather than as exactly one adapter behind a generic the
/// way each repository port is.
#[async_trait]
pub trait WebhookVerifier: Send + Sync {
    /// Verifies `payload` against `signature_header` and records the event,
    /// returning whether this is the first delivery.
    ///
    /// Takes the body as **bytes**, not `&str`: a non-UTF-8 body becomes a
    /// typed rejection, not a panic. Takes **no secret** — the
    /// implementation holds the signing secret, so `api` never handles one.
    ///
    /// # Host preconditions
    ///
    /// The route that calls this (in `api`) is responsible for
    /// two things this port cannot check for itself:
    ///
    /// - **Pass the raw, untouched request body.** The signature is an HMAC
    ///   over exactly the bytes Stripe sent, so any re-encoding,
    ///   pretty-printing or key reordering between the socket and this call
    ///   breaks verification. Read the body as bytes
    ///   and forward it unmodified.
    /// - **Bound the body size before calling.** Nothing here caps the input
    ///   it will HMAC. The limit belongs on the route (e.g. Axum's
    ///   `DefaultBodyLimit`), because an implementation of this port never
    ///   reads from the network.
    async fn verify_and_record(
        &self,
        payload: &[u8],
        signature_header: &str,
    ) -> Result<WebhookReceipt, DomainError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compiles only if `WebhookVerifier` is dyn-compatible — the property
    /// the `#[async_trait]` decision exists for.
    #[allow(dead_code)]
    fn assert_dyn_compatible(_verifier: &dyn WebhookVerifier) {}

    /// `Duplicate` exposes no `VerifiedEvent`: the only thing a caller can
    /// read off it is the echoed event id. This mostly documents intent —
    /// the guarantee is structural, enforced by the variant's definition.
    #[test]
    fn duplicate_carries_no_verified_event() {
        let receipt = WebhookReceipt::Duplicate {
            stripe_event_id: "evt_123".to_string(),
        };
        assert!(matches!(
            receipt,
            WebhookReceipt::Duplicate { stripe_event_id } if stripe_event_id == "evt_123"
        ));
    }
}
