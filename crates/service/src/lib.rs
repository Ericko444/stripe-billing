//! Use cases orchestrating the domain ports (`init-spec.md` §8.2).
//!
//! Performs **no I/O of its own** -- every effect goes through a `domain`
//! port, which is why the whole crate tests with Docker off. Webhook
//! processing (the `WebhookProcessor`, covering every `init-spec.md` §10.4
//! event type) carries two rules worth stating up front because they are
//! easy to get backwards:
//!
//! - the mirror write always happens **before** the host's
//!   `BillingEventSink` is called (§8.3: the mirror is what Stripe said, the
//!   sink is a side effect of it); and
//! - a `WebhookEvent` is marked processed **only once both** that write and
//!   the sink call have succeeded -- a sink failure leaves `processed_at`
//!   NULL for a later recovery sweep (§10.2, §8.3).
//!
//! Tenant is always resolved from the database (`stripe_customer_id` -> a
//! `Customer` -> its `TenantId`), never from the payload. The lone
//! exception is `checkout_session`'s bootstrap fallback, which is a
//! separately named, separately documented branch reached only when no
//! local customer row exists (§10.3).

mod checkout_session;
mod invoice_events;
mod payment_method_events;
mod reads;
mod subscription_lifecycle;
mod webhook;

#[cfg(test)]
mod test_support;

pub use reads::{ReadService, Reads};
pub use webhook::{EventOutcome, NotAppliedReason, WebhookHandler, WebhookProcessor};
