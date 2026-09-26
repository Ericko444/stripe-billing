use async_trait::async_trait;
use time::OffsetDateTime;

use crate::{DomainError, SubscriptionStatus, TenantId};

/// Inputs for creating a Stripe customer. Every field here varies the Stripe
/// request body, which is why the set is kept minimal: `stripe-adapter`
/// derives the idempotency fingerprint from exactly these fields, so an
/// input added here without also reaching the fingerprint would let two
/// different requests share a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateCustomerParams {
    /// The customer's email, if known.
    pub email: Option<String>,
    /// The customer's display name, if known.
    pub name: Option<String>,
}

/// Inputs for updating a Stripe customer. A `None` field is left unchanged
/// on Stripe's side, matching Stripe's own partial-update semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateCustomerParams {
    /// The new email, or `None` to leave it unchanged.
    pub email: Option<String>,
    /// The new display name, or `None` to leave it unchanged.
    pub name: Option<String>,
}

/// What Stripe returned about a customer, in domain terms. A named struct
/// rather than a bare `String` so a field can be added later (e.g.
/// `default_payment_method`) without changing the trait's signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerSnapshot {
    /// The Stripe customer id.
    pub stripe_customer_id: String,
}

/// Whether a subscription cancellation takes effect at the end of the
/// current billing period or immediately. A two-variant enum rather than a
/// `bool` so a call site cannot silently invert it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancellationTiming {
    /// The subscription stays active until `current_period_end`, then ends.
    AtPeriodEnd,
    /// The subscription ends immediately.
    Immediate,
}

/// What Stripe returned about a subscription, in domain terms. Deliberately
/// not the `Subscription` aggregate: Stripe supplies none of the local ids
/// (`SubscriptionId`, `CustomerId`, `PlanId`) or `deleted_at`, so returning
/// the aggregate would mean the adapter inventing local state. For
/// anything Stripe owns, Stripe is authoritative and the local tables are a
/// queryable cache; the module never invents billing state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionSnapshot {
    /// The Stripe subscription id.
    pub stripe_subscription_id: String,
    /// The Stripe subscription *item* id -- stored so a later plan change
    /// updates this item rather than adding a second one, which would bill
    /// the customer twice.
    pub stripe_subscription_item_id: String,
    /// The subscription's current lifecycle status.
    pub status: SubscriptionStatus,
    /// Start of the current billing period.
    pub current_period_start: OffsetDateTime,
    /// End of the current billing period.
    pub current_period_end: OffsetDateTime,
    /// Whether the subscription is set to end at the period boundary.
    pub cancel_at_period_end: bool,
}

/// What Stripe returned about a newly created SetupIntent, in domain terms.
///
/// One field: the SetupIntent's `client_secret`. It is **browser-destined**
/// -- the frontend uses it with Stripe.js to collect and confirm a payment
/// method against this customer -- and it is **bearer-ish**: whoever holds
/// it can attach a payment method to that customer. It travels in the API
/// response body and **must never reach a log line**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupIntentSnapshot {
    /// The SetupIntent's `client_secret` (`seti_..._secret_...`). Never log
    /// this.
    pub client_secret: String,
}

/// Inputs for creating a Stripe Checkout Session in `subscription` mode.
///
/// The two URLs are **not** caller-supplied -- the host sets them once when
/// it constructs `api`'s `AppState`. A caller that could set `success_url`
/// would have an open redirect in the one flow where the customer is most
/// primed to trust the destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutSessionParams {
    /// The Stripe customer the session subscribes.
    pub stripe_customer_id: String,
    /// The Stripe price the subscription is for.
    pub stripe_price_id: String,
    /// Where Stripe returns the customer after a completed checkout.
    pub success_url: String,
    /// Where Stripe returns the customer if they abandon checkout.
    pub cancel_url: String,
}

/// What Stripe returned about a newly created Checkout Session, in domain
/// terms.
///
/// Two fields. `url` is **browser-destined** -- it is the hosted checkout
/// page the frontend redirects to -- and it travels in the API response
/// body and **must never reach a log line**, the same rule as
/// [`SetupIntentSnapshot`]'s `client_secret`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutSessionSnapshot {
    /// The hosted Checkout page URL (`https://checkout.stripe.com/...`).
    /// Never log this.
    pub url: String,
    /// The Stripe Checkout Session id (`cs_...`).
    pub stripe_session_id: String,
}

/// Port for mutating Stripe's customer, subscription and payment-method
/// state. Implemented by the `stripe-adapter` crate; no I/O here.
///
/// Unlike the repository ports, this trait uses `#[async_trait]` rather than
/// native `async fn`, because it needs to be dyn-compatible: it is held by
/// shared application state that a host wires at runtime (a
/// `dyn BillingProvider` in `api`'s `AppState`), not by exactly one
/// adapter the way each repository port is.
///
/// Grown method-by-method as use cases needed it, not toward a speculative
/// full surface: the customer and subscription methods first, then the
/// SetupIntent, Checkout Session and payment-method methods the write
/// routes need.
#[async_trait]
pub trait BillingProvider: Send + Sync {
    /// Creates a Stripe customer for the given tenant.
    async fn create_customer(
        &self,
        tenant_id: TenantId,
        params: CreateCustomerParams,
    ) -> Result<CustomerSnapshot, DomainError>;

    /// Updates an existing Stripe customer.
    async fn update_customer(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: &str,
        params: UpdateCustomerParams,
    ) -> Result<CustomerSnapshot, DomainError>;

    /// Creates a subscription for a customer against a price.
    async fn create_subscription(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: &str,
        stripe_price_id: &str,
    ) -> Result<SubscriptionSnapshot, DomainError>;

    /// Changes an existing subscription's plan by updating its subscription
    /// item -- never by adding a second item, which would bill twice.
    /// Prorates the change (`proration_behavior=create_prorations`), so the
    /// customer is credited or charged for the unused part of the period.
    async fn change_plan(
        &self,
        tenant_id: TenantId,
        stripe_subscription_id: &str,
        stripe_subscription_item_id: &str,
        new_stripe_price_id: &str,
    ) -> Result<SubscriptionSnapshot, DomainError>;

    /// Cancels a subscription, either at the end of the current period or
    /// immediately.
    async fn cancel_subscription(
        &self,
        tenant_id: TenantId,
        stripe_subscription_id: &str,
        timing: CancellationTiming,
    ) -> Result<SubscriptionSnapshot, DomainError>;

    /// Creates a SetupIntent for an existing Stripe customer, so the
    /// frontend can collect and confirm a payment method against them.
    ///
    /// The returned [`SetupIntentSnapshot`] carries only the
    /// `client_secret`, which is browser-destined and **must never be
    /// logged** -- see that type's docs. The caller (`service`) resolves the
    /// tenant to a `stripe_customer_id` first (via `ensure_customer`); this
    /// method never creates a customer of its own.
    async fn create_setup_intent(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: &str,
    ) -> Result<SetupIntentSnapshot, DomainError>;

    /// Sets `stripe_payment_method_id` as the customer's default for
    /// invoices and subscriptions
    /// (`invoice_settings.default_payment_method` on the customer). Returns
    /// nothing -- the caller already knows which method it asked for, and the
    /// local mirror is reconciled by `service`, not from a snapshot here.
    ///
    /// **Ordering: the caller runs this *before* touching the mirror.**
    /// Reversed, a failure here would leave the local `is_default` flags
    /// disagreeing with Stripe. `service`'s test suite
    /// pins the order with a double that fails this call and asserts the
    /// flags did not move.
    async fn set_default_payment_method(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: &str,
        stripe_payment_method_id: &str,
    ) -> Result<(), DomainError>;

    /// Detaches a payment method from its customer
    /// (`POST /v1/payment_methods/{id}/detach`). Permanent and irreversible
    /// at Stripe; the local mirror is *soft*-deleted by `service`
    /// afterwards.
    ///
    /// **Ordering: Stripe first, then the mirror.**
    /// Reversed, a Stripe failure would leave the mirror claiming the card
    /// is gone while it is still attached and still billable -- the customer
    /// sees "removed" and keeps being charged.
    ///
    /// The idempotency fingerprint is **tenant + payment method id only** --
    /// no timestamp. A retry inside the key window replays the original
    /// detach rather than issuing a fresh one, so it cannot detach a card
    /// the customer has since re-added.
    async fn detach_payment_method(
        &self,
        tenant_id: TenantId,
        stripe_payment_method_id: &str,
    ) -> Result<(), DomainError>;

    /// Creates a Stripe Checkout Session in `subscription` mode for the
    /// customer and price in `params`, and returns its hosted-page `url`.
    ///
    /// The caller (`service`) resolves the tenant to a `stripe_customer_id`
    /// (via `ensure_customer`) and the local plan to a `stripe_price_id`
    /// first; the two URLs come from host config, never the request. The
    /// returned [`CheckoutSessionSnapshot`]'s `url` is browser-destined and
    /// **must never be logged** -- see that type's docs.
    async fn create_checkout_session(
        &self,
        tenant_id: TenantId,
        params: CheckoutSessionParams,
    ) -> Result<CheckoutSessionSnapshot, DomainError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compiles only if `BillingProvider` is dyn-compatible -- the property
    /// the `#[async_trait]` decision exists for.
    #[allow(dead_code)]
    fn assert_dyn_compatible(_provider: &dyn BillingProvider) {}

    #[test]
    fn cancellation_timing_variants_are_distinct() {
        assert_ne!(
            CancellationTiming::AtPeriodEnd,
            CancellationTiming::Immediate
        );
    }
}
