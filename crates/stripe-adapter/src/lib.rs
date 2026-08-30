//! Stripe adapter: the outbound `BillingProvider` path and the inbound
//! webhook path, with everything each needs.
//!
//! **Outbound** — customer and subscription writes to Stripe: client
//! construction with a base-URL override ([`build_client`]), the
//! [`StripeError`] taxonomy and its flattening to `domain::DomainError`, the
//! request [`fingerprint`], the reserve/complete state machine ([`Ledger`]),
//! and [`StripeBillingProvider`], which wires them together behind
//! `domain::BillingProvider`.
//!
//! **Inbound** — verified, deduplicated webhook receipt behind
//! `domain::WebhookVerifier`: hand-rolled signature verification
//! (`webhook_signature`, accepting *any* matching `v1` so an endpoint-secret
//! rotation never rejects a valid delivery), the [`WebhookError`] taxonomy
//! (which flattens to *nothing* — see its docs), [`WebhookConfig`], and
//! [`StripeWebhookVerifier`], which joins verification to the
//! `billing.webhook_events` dedup ledger. It writes only that ledger, never
//! a mirror table.
//!
//! **The one hard rule for every mutating call:** send it with
//! `stripe::RequestStrategy::Idempotent(key)` and nothing else, where `key`
//! was reserved and persisted by the [`Ledger`] *before* the call. The SDK's
//! `idempotent_with_uuid()`, `IdempotencyKey::new_uuid_v4()`,
//! `RequestStrategy::Retry` and `RequestStrategy::ExponentialBackoff` each
//! mint a fresh uuid per attempt, which is exactly the non-idempotency this
//! crate exists to prevent — none of them are used here.
//!
//! **The two hard rules for every webhook receipt:** *verify before parse* —
//! nothing deserializes the body until the signature has passed — and
//! *record before returning `Fresh`* — the caller is never told to process
//! an event that is not yet in the ledger. Both are pinned by
//! `tests/webhooks.rs`, the second by a zero-rows assertion on every
//! rejection path.

mod client;
mod config;
mod customers;
mod error;
mod fingerprint;
mod ledger;
mod provider;
mod subscriptions;
mod webhook;
mod webhook_error;
mod webhook_signature;

/// Test-only signing helpers, shared between this crate's unit tests and its
/// integration tests. Not part of the supported API.
#[doc(hidden)]
pub mod test_support;

pub use client::build_client;
pub use config::{StripeConfig, WebhookConfig, pinned_api_version};
pub use error::StripeError;
pub use fingerprint::fingerprint;
pub use ledger::{Ledger, Reservation};
pub use provider::StripeBillingProvider;
pub use webhook::StripeWebhookVerifier;
pub use webhook_error::WebhookError;
pub use webhook_signature::DEFAULT_TOLERANCE;
