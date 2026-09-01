//! Use cases orchestrating the domain ports (`init-spec.md` §8.2).
//!
//! Performs no I/O of its own -- every effect goes through a `domain` port.
//! The one use case here so far, webhook processing, carries two rules worth
//! stating up front because they are easy to get backwards: the mirror
//! write always happens before the host's `BillingEventSink` is called, and
//! a `WebhookEvent` is marked processed only once both that write and the
//! sink call have succeeded (`init-spec.md` §10.2, §8.3).

mod invoice_events;
mod payment_method_events;
mod subscription_lifecycle;
mod webhook;

#[cfg(test)]
mod test_support;

pub use webhook::{EventOutcome, NotAppliedReason, WebhookHandler, WebhookProcessor};
