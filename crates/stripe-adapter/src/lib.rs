//! Stripe adapter: the `BillingProvider` implementation and its idempotency
//! ledger.
//!
//! This crate owns everything that talks to Stripe for customer and
//! subscription writes: client construction with a base-URL override
//! ([`build_client`]), the [`StripeError`] taxonomy and its flattening to
//! `domain::DomainError`, the request [`fingerprint`], the reserve/complete
//! state machine ([`Ledger`]), and [`StripeBillingProvider`], which wires
//! them together behind `domain::BillingProvider`. Webhook verification and
//! parsing is Phase 3 and lives elsewhere.
//!
//! **The one hard rule for every mutating call:** send it with
//! `stripe::RequestStrategy::Idempotent(key)` and nothing else, where `key`
//! was reserved and persisted by the [`Ledger`] *before* the call. The SDK's
//! `idempotent_with_uuid()`, `IdempotencyKey::new_uuid_v4()`,
//! `RequestStrategy::Retry` and `RequestStrategy::ExponentialBackoff` each
//! mint a fresh uuid per attempt, which is exactly the non-idempotency this
//! crate exists to prevent — none of them are used here.

mod client;
mod config;
mod customers;
mod error;
mod fingerprint;
mod ledger;
mod provider;
mod subscriptions;

pub use client::build_client;
pub use config::{StripeConfig, pinned_api_version};
pub use error::StripeError;
pub use fingerprint::fingerprint;
pub use ledger::{Ledger, Reservation};
pub use provider::StripeBillingProvider;
