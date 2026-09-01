use async_trait::async_trait;
use domain::{
    BillingProvider, CancellationTiming, CreateCustomerParams, CustomerSnapshot, DomainError,
    OutboundRequestRepository, SetupIntentSnapshot, SubscriptionSnapshot, TenantId,
    UpdateCustomerParams,
};

use crate::ledger::Ledger;
use crate::{StripeConfig, StripeError, build_client, customers, setup_intents, subscriptions};

/// Stripe-backed implementation of `domain::BillingProvider`: fingerprint
/// every mutating call, reserve and persist an idempotency key through
/// `Ledger<R>` *before* the call, then send it with
/// `RequestStrategy::Idempotent`.
///
/// Generic over `R` for the same reason `Ledger<R>` is: the repository
/// ports use `impl Future`-returning methods, not `dyn`, and this type's
/// ledger is held by value.
///
/// Every method exists twice with identical signatures: inherently (usable
/// on the concrete type without importing the trait) and through the
/// `#[async_trait] impl BillingProvider` below, which a host that wires a
/// `dyn BillingProvider` at runtime needs. `#[async_trait]` boxes its
/// futures as `Send`, which is why `R` carries `Send + Sync` there and why
/// `OutboundRequestRepository`'s methods are declared `-> impl Future + Send`
/// (see that port's own doc comment). The trait impl is pure delegation --
/// one body per operation, in `customers.rs` / `subscriptions.rs`.
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

    /// Creates a SetupIntent for an existing Stripe customer. Same signature
    /// as `BillingProvider::create_setup_intent`.
    pub async fn create_setup_intent(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: &str,
    ) -> Result<SetupIntentSnapshot, DomainError> {
        setup_intents::create_setup_intent(
            &self.client,
            &self.ledger,
            tenant_id,
            stripe_customer_id,
        )
        .await
    }
}

/// The port impl a host wires behind `dyn BillingProvider`. Each method is a
/// one-line delegation to the inherent method of the same name above; the
/// real work lives in `customers.rs` and `subscriptions.rs`. `R: Send + Sync`
/// because `#[async_trait]` boxes these futures as `Send`.
#[async_trait]
impl<R: OutboundRequestRepository + Send + Sync> BillingProvider for StripeBillingProvider<R> {
    async fn create_customer(
        &self,
        tenant_id: TenantId,
        params: CreateCustomerParams,
    ) -> Result<CustomerSnapshot, DomainError> {
        StripeBillingProvider::create_customer(self, tenant_id, params).await
    }

    async fn update_customer(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: &str,
        params: UpdateCustomerParams,
    ) -> Result<CustomerSnapshot, DomainError> {
        StripeBillingProvider::update_customer(self, tenant_id, stripe_customer_id, params).await
    }

    async fn create_subscription(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: &str,
        stripe_price_id: &str,
    ) -> Result<SubscriptionSnapshot, DomainError> {
        StripeBillingProvider::create_subscription(
            self,
            tenant_id,
            stripe_customer_id,
            stripe_price_id,
        )
        .await
    }

    async fn change_plan(
        &self,
        tenant_id: TenantId,
        stripe_subscription_id: &str,
        stripe_subscription_item_id: &str,
        new_stripe_price_id: &str,
    ) -> Result<SubscriptionSnapshot, DomainError> {
        StripeBillingProvider::change_plan(
            self,
            tenant_id,
            stripe_subscription_id,
            stripe_subscription_item_id,
            new_stripe_price_id,
        )
        .await
    }

    async fn cancel_subscription(
        &self,
        tenant_id: TenantId,
        stripe_subscription_id: &str,
        timing: CancellationTiming,
    ) -> Result<SubscriptionSnapshot, DomainError> {
        StripeBillingProvider::cancel_subscription(self, tenant_id, stripe_subscription_id, timing)
            .await
    }

    async fn create_setup_intent(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: &str,
    ) -> Result<SetupIntentSnapshot, DomainError> {
        StripeBillingProvider::create_setup_intent(self, tenant_id, stripe_customer_id).await
    }
}
