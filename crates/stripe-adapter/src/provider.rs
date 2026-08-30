use domain::{
    CancellationTiming, CreateCustomerParams, CustomerSnapshot, DomainError,
    OutboundRequestRepository, SubscriptionSnapshot, TenantId, UpdateCustomerParams,
};

use crate::ledger::Ledger;
use crate::{StripeConfig, StripeError, build_client, customers, subscriptions};

/// Stripe-backed implementation of `domain::BillingProvider`: fingerprint
/// every mutating call, reserve and persist an idempotency key through
/// `Ledger<R>` *before* the call, then send it with
/// `RequestStrategy::Idempotent`.
///
/// Generic over `R` for the same reason `Ledger<R>` is: native `async fn`
/// in `OutboundRequestRepository` is not dyn-compatible, and this type's
/// ledger is held by value, not behind a trait object.
///
/// All five methods exist as inherent `async fn`s with exactly the
/// `BillingProvider` signatures. The blanket
/// `#[async_trait] impl<R> BillingProvider for StripeBillingProvider<R>` is
/// **not** written here: `#[async_trait]` requires every awaited future to
/// be `Send`, and `OutboundRequestRepository`'s methods are native
/// `async fn` under `#[allow(async_fn_in_trait)]`, which gives no `Send`
/// guarantee for a generic `R`. Wiring the trait impl therefore needs the
/// repository port to promise `Send` futures (return-position
/// `impl Future + Send`, or `trait_variant`) -- a Phase 1 signature change
/// the Phase 2 spec says to raise before making. Until then a host wanting
/// a `dyn BillingProvider` wraps the concrete type in a thin newtype in the
/// crate that owns the concrete `R` (Phase 4's `demo`), where the future's
/// `Send`-ness is provable.
pub struct StripeBillingProvider<R: OutboundRequestRepository> {
    client: stripe::Client,
    ledger: Ledger<R>,
}

impl<R: OutboundRequestRepository> StripeBillingProvider<R> {
    /// Builds a provider from a `StripeConfig` and a ledger repository.
    pub fn new(config: &StripeConfig, repo: R) -> Result<Self, StripeError> {
        let client = build_client(config)?;
        Ok(Self {
            client,
            ledger: Ledger::new(repo),
        })
    }

    /// Creates a Stripe customer for the given tenant. Same signature as
    /// `BillingProvider::create_customer`.
    pub async fn create_customer(
        &self,
        tenant_id: TenantId,
        params: CreateCustomerParams,
    ) -> Result<CustomerSnapshot, DomainError> {
        customers::create_customer(&self.client, &self.ledger, tenant_id, params).await
    }

    /// Updates an existing Stripe customer. Same signature as
    /// `BillingProvider::update_customer`.
    pub async fn update_customer(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: &str,
        params: UpdateCustomerParams,
    ) -> Result<CustomerSnapshot, DomainError> {
        customers::update_customer(
            &self.client,
            &self.ledger,
            tenant_id,
            stripe_customer_id,
            params,
        )
        .await
    }

    /// Creates a subscription for a customer against a price. Same signature
    /// as `BillingProvider::create_subscription`.
    pub async fn create_subscription(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: &str,
        stripe_price_id: &str,
    ) -> Result<SubscriptionSnapshot, DomainError> {
        subscriptions::create_subscription(
            &self.client,
            &self.ledger,
            tenant_id,
            stripe_customer_id,
            stripe_price_id,
        )
        .await
    }

    /// Changes an existing subscription's plan by updating its stored
    /// subscription item. Same signature as `BillingProvider::change_plan`.
    pub async fn change_plan(
        &self,
        tenant_id: TenantId,
        stripe_subscription_id: &str,
        stripe_subscription_item_id: &str,
        new_stripe_price_id: &str,
    ) -> Result<SubscriptionSnapshot, DomainError> {
        subscriptions::change_plan(
            &self.client,
            &self.ledger,
            tenant_id,
            stripe_subscription_id,
            stripe_subscription_item_id,
            new_stripe_price_id,
        )
        .await
    }

    /// Cancels a subscription, at the period end or immediately. Same
    /// signature as `BillingProvider::cancel_subscription`.
    pub async fn cancel_subscription(
        &self,
        tenant_id: TenantId,
        stripe_subscription_id: &str,
        timing: CancellationTiming,
    ) -> Result<SubscriptionSnapshot, DomainError> {
        subscriptions::cancel_subscription(
            &self.client,
            &self.ledger,
            tenant_id,
            stripe_subscription_id,
            timing,
        )
        .await
    }
}
