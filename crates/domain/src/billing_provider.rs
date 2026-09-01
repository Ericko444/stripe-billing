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
/// rather than a bare `String` so a later phase can add a field (e.g.
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
/// the aggregate would mean the adapter inventing local state -- exactly
/// what `init-spec.md` §7.4 forbids ("Stripe is authoritative, local tables
/// are a cache").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionSnapshot {
    /// The Stripe subscription id.
    pub stripe_subscription_id: String,
    /// The Stripe subscription *item* id -- stored so a later plan change
    /// updates this item rather than adding a second one (`init-spec.md`
    /// §5.1).
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
/// response body and **must never reach a log line** (`init-spec.md` §15's
/// Never list; `docs/spec/phase-4c-write-routes.md` §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupIntentSnapshot {
    /// The SetupIntent's `client_secret` (`seti_..._secret_...`). Never log
    /// this.
    pub client_secret: String,
}

/// Port for mutating Stripe's customer, subscription and payment-method
/// state. Implemented by the `stripe-adapter` crate; no I/O here.
///
/// Unlike the repository ports, this trait uses `#[async_trait]` rather than
/// native `async fn`, because it needs to be dyn-compatible: it is held by
/// shared application state that a host wires at runtime (a
/// `dyn BillingProvider` in `api`'s `AppState`), not by exactly one
/// adapter the way each repository port is. See `docs/intent/phase-2.md`
/// for the full argument.
///
/// Grows method-by-method as later phases need it, not toward a speculative
/// full surface: Phase 2 brought the customer and subscription methods;
/// Phase 4c adds the SetupIntent, Checkout Session and payment-method
/// methods the write routes need.
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
    /// item -- never by adding a second item (`init-spec.md` §5.1, §7.3).
    /// Prorates the change (`proration_behavior=create_prorations`).
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
