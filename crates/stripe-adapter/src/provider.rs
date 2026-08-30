use domain::{
    CreateCustomerParams, CustomerSnapshot, DomainError, OutboundRequestRepository, TenantId,
    UpdateCustomerParams,
};

use crate::ledger::Ledger;
use crate::{StripeConfig, StripeError, build_client, customers};

/// Stripe-backed implementation of the pieces of `domain::BillingProvider`
/// built so far.
///
/// Generic over `R` for the same reason `Ledger<R>` is: native `async fn`
/// in `OutboundRequestRepository` is not dyn-compatible, and this type's
/// ledger is held by value, not behind a trait object.
///
/// **Not yet `impl domain::BillingProvider`.** That trait's five methods
/// are declared as a unit (`domain::billing_provider.rs`), but this crate
/// builds them one at a time (Tasks 15, 17-20). Implementing the trait now
/// would mean stubbing the other four with `unimplemented!()` -- a bare
/// panic on a path nothing yet prevents from being called, which is
/// exactly what S6 forbids on any nominal path. The inherent
/// `create_customer` below has the same signature the trait method will
/// have; wiring `impl BillingProvider for StripeBillingProvider<R>` is a
/// mechanical step once all five exist.
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
}
