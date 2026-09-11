use async_trait::async_trait;
use domain::{
    DomainError, VerifiedEvent, WebhookEvent, WebhookEventRepository, WebhookReceipt,
    WebhookVerifier,
};
use secrecy::ExposeSecret;
use serde_json::Value;
use time::OffsetDateTime;

use crate::config::WebhookConfig;
use crate::webhook_error::WebhookError;
use crate::webhook_signature;

/// Verifies inbound Stripe webhooks and records them in the dedup ledger,
/// behind `domain::WebhookVerifier`.
///
/// Holds the signing secret (inside [`WebhookConfig`]) so a caller — `api`,
/// reaching this through a `dyn WebhookVerifier` — never does, plus a
/// `WebhookEventRepository` for the single ledger write.
/// Generic over `R` like `StripeBillingProvider<R>`, for the same reason:
/// exactly one adapter implements the repository port, and `demo` picks it.
///
/// Two orderings in [`verify_and_record`](Self::verify_and_record) carry the
/// whole design, and the integration tests assert both:
///
/// - **Verify before parse.** Nothing deserializes the body until the
///   signature has passed — the `serde_json::from_str` call sits *after*
///   `webhook_signature::verify`.
/// - **Record before returning `Fresh`.** The caller is never told to
///   process an event that is not yet in the ledger.
pub struct StripeWebhookVerifier<R> {
    config: WebhookConfig,
    repo: R,
}

impl<R> StripeWebhookVerifier<R> {
    /// Wraps a `WebhookConfig` (the signing secret and the replay tolerance)
    /// and a repository for the ledger write.
    pub fn new(config: WebhookConfig, repo: R) -> Self {
        Self { config, repo }
    }
}

#[async_trait]
impl<R: WebhookEventRepository + Send + Sync> WebhookVerifier for StripeWebhookVerifier<R> {
    async fn verify_and_record(
        &self,
        payload: &[u8],
        signature_header: &str,
    ) -> Result<WebhookReceipt, DomainError> {
        // Bytes in at the port; a non-UTF-8 body is a typed
        // rejection here, never a panic.
        let body =
            std::str::from_utf8(payload).map_err(|e| WebhookError::Payload(e.to_string()))?;

        // Nothing below this line runs until the signature verifies.
        webhook_signature::verify(
            body,
            signature_header,
            self.config.signing_secret.expose_secret(),
            self.config.tolerance,
            OffsetDateTime::now_utc(),
        )?;

        // Our own envelope read: the three fields the ledger needs, plus the
        // payload verbatim. No typed Stripe event and no match on the event
        // type — an unrecognised type takes the identical path.
        let envelope: Value =
            serde_json::from_str(body).map_err(|e| WebhookError::Payload(e.to_string()))?;
        let (stripe_event_id, event_type, created) = read_envelope(&envelope)?;

        // Insert-and-catch-conflict, not check-then-insert: the duplicate
        // signal is the unique-violation `Conflict` from `create`, which
        // handles concurrent redelivery by construction. `tenant_id` is
        // None — `service` resolves it from the event after receipt, and
        // `WebhookEvent.tenant_id` is `Option` for exactly this.
        match self
            .repo
            .create(None, stripe_event_id.clone(), event_type, envelope)
            .await
        {
            Ok(row) => Ok(WebhookReceipt::Fresh(to_verified_event(row, created))),
            Err(DomainError::Conflict) => Ok(WebhookReceipt::Duplicate { stripe_event_id }),
            Err(other) => Err(other),
        }
    }
}

/// Reads `id`, `type` and `created` from a Stripe event envelope. `created`
/// is Stripe's Unix-seconds timestamp. A missing or wrong-typed field is a
/// [`WebhookError::Payload`] — the body verified, but it is not the shape an
/// event envelope has.
fn read_envelope(envelope: &Value) -> Result<(String, String, OffsetDateTime), WebhookError> {
    let field_str = |name: &str| -> Result<String, WebhookError> {
        envelope
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| WebhookError::Payload(format!("event envelope missing string `{name}`")))
    };

    let stripe_event_id = field_str("id")?;
    let event_type = field_str("type")?;

    let created_unix = envelope
        .get("created")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            WebhookError::Payload("event envelope missing integer `created`".to_string())
        })?;
    let created = OffsetDateTime::from_unix_timestamp(created_unix)
        .map_err(|e| WebhookError::Payload(format!("event `created` out of range: {e}")))?;

    Ok((stripe_event_id, event_type, created))
}

/// Builds the domain `VerifiedEvent` from the freshly-inserted ledger row
/// and Stripe's own event timestamp. `created` comes from the envelope, not
/// the row: the row's `created_at` is when *we* received the event, whereas
/// `VerifiedEvent.created` is Stripe's timestamp — carried through, never
/// compared.
fn to_verified_event(row: WebhookEvent, created: OffsetDateTime) -> VerifiedEvent {
    VerifiedEvent {
        id: row.id,
        stripe_event_id: row.stripe_event_id,
        event_type: row.event_type,
        created,
        payload: row.payload,
    }
}
