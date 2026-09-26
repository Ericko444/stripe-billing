//! Use cases orchestrating the domain ports.
//!
//! Performs **no I/O of its own** -- every effect goes through a `domain`
//! port, which is why the whole crate tests with Docker off. Webhook
//! processing (the `WebhookProcessor`, covering every handled event type)
//! carries two rules worth stating up front because they are easy to get
//! backwards:
//!
//! - the mirror write always happens **before** the host's
//!   `BillingEventSink` is called (the mirror is what Stripe said, the sink
//!   is a side effect of it); and
//! - a `WebhookEvent` is marked processed **only once both** that write and
//!   the sink call have succeeded -- a sink failure leaves `processed_at`
//!   NULL for a later recovery sweep.
//!
//! Tenant is always resolved from the database (`stripe_customer_id` -> a
//! `Customer` -> its `TenantId`), never from the payload. The lone
//! exception is `checkout_session`'s bootstrap fallback, which is a
//! separately named, separately documented branch reached only when no
//! local customer row exists.
//!
//! # The write path (`Writes`, `writes.rs`)
//!
//! [`Writes`] is the mirror of [`Reads`]: an object-safe façade, held as
//! `Arc<dyn Writes>` on `api`'s `AppState`, one method per mutating route.
//! Unlike the read path it holds a `BillingProvider`, so **every method
//! calls out to Stripe**. Three rules run through all of them (the method
//! rustdocs in `writes.rs` and on the `domain` port carry the full
//! argument):
//!
//! - **Ownership before the outbound call.** Every id is resolved against a
//!   *tenant-scoped* repository `find` first; an id that is unknown or
//!   belongs to another tenant is `DomainError::NotFound` (a 404 identical
//!   to an unknown id) with **no call to Stripe**. On a write route a
//!   cross-tenant id would be a *mutation* on data the caller does not own,
//!   not merely a disclosure.
//! - **Stripe first, then the mirror** for `set_default_payment_method` and
//!   `remove_payment_method`. A failed Stripe call returns before the
//!   local row moves, so the mirror never claims a card is the default, or
//!   is gone, while Stripe disagrees. `service`'s tests pin the order with a
//!   double that fails the Stripe call and asserts the local row is
//!   unchanged -- the assertion a reversed (mirror-first) implementation
//!   fails.
//! - **The snapshot goes through the webhook ordering guard.** After
//!   `change_plan` / `cancel_subscription`, Stripe's returned
//!   `SubscriptionSnapshot` is applied through the same ordering guard the
//!   webhook path uses (`SubscriptionRepository::apply_event`, or the
//!   equivalent predicate inside `change_plan`'s merged transaction -- see
//!   that method's own rustdoc for why the two writes had to merge), so the
//!   API response and the `customer.subscription.updated` webhook that races
//!   it cannot regress the row. `plan_id` is the one column no webhook
//!   writes, moved by `change_plan`'s own unguarded half (or the standalone
//!   `set_plan`, still used directly nowhere in production but kept and
//!   tested as a primitive).
//!
//! `ensure_customer` is routeless: `create_setup_intent` and
//! `start_checkout_session` call it to resolve (or create) the tenant's
//! Stripe customer before naming it to Stripe.

mod checkout_session;
mod context;
mod invoice_events;
mod payment_method_events;
mod reads;
mod subscription_lifecycle;
mod webhook;
mod writes;

#[cfg(test)]
mod test_support;

pub use context::RequestContext;
pub use reads::{ReadService, Reads};
pub use webhook::{EventOutcome, NotAppliedReason, WebhookHandler, WebhookProcessor};
pub use writes::{WriteService, Writes};
