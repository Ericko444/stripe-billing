use std::future::Future;

use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DomainError, TenantId};

/// Identifies a `Customer`. Distinct from other entities' ids so the
/// compiler rejects passing the wrong id where a customer id is expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CustomerId(Uuid);

impl CustomerId {
    /// Wraps a raw `Uuid` as a `CustomerId`.
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the underlying `Uuid`.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

/// Binds a tenant to a Stripe customer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Customer {
    /// The customer's id.
    pub id: CustomerId,
    /// The tenant this customer belongs to.
    pub tenant_id: TenantId,
    /// The linked Stripe customer id, if one has been created.
    pub stripe_customer_id: Option<String>,
    /// When the customer was created.
    pub created_at: OffsetDateTime,
    /// When the customer was soft-deleted, if at all.
    pub deleted_at: Option<OffsetDateTime>,
}

/// Port for persisting and querying `Customer` records. Implemented by an
/// adapter crate (`persistence`); no I/O here.
///
/// This trait is meant to be used generically (`fn new<R: CustomerRepository>
/// (repo: R)`), never as `dyn CustomerRepository`. Its methods are still
/// written as `fn … -> impl Future<Output = …> + Send` rather than bare
/// `async fn`, matching `OutboundRequestRepository` and
/// `WebhookEventRepository`: a later phase's `WebhookProcessor<C, S, W, K>`
/// goes behind `#[async_trait]` to implement the object-safe
/// `WebhookHandler` port, which boxes its futures as `Send`, so every future
/// it awaits -- including these -- must be `Send` too. A bare `async fn` in a
/// trait does not promise that for a generic `C`. Implementors may still
/// write `async fn` in the `impl` block; the bound is checked there.
pub trait CustomerRepository {
    /// Creates a new customer for the given tenant.
    fn create(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: Option<String>,
    ) -> impl Future<Output = Result<Customer, DomainError>> + Send;

    /// Finds a customer by id, scoped to the tenant. Returns `None` if the
    /// customer does not exist or has been soft-deleted.
    fn find(
        &self,
        tenant_id: TenantId,
        id: CustomerId,
    ) -> impl Future<Output = Result<Option<Customer>, DomainError>> + Send;

    /// Lists all customers for the given tenant, excluding soft-deleted ones.
    fn list(
        &self,
        tenant_id: TenantId,
    ) -> impl Future<Output = Result<Vec<Customer>, DomainError>> + Send;

    /// Resolves the customer -- and therefore the tenant -- that owns a
    /// Stripe customer id.
    ///
    /// **The one method on a tenant-scoped entity's repository that does not
    /// take a `TenantId`.** (`WebhookEventRepository`'s methods take none
    /// either, but the webhook ledger is not a tenant-scoped entity to begin
    /// with, which is why none of its methods do.) This one is deliberately
    /// the exception: a later phase's webhook path (`init-spec.md` §10.3)
    /// has no token and no tenant context -- the Stripe signature is the
    /// authentication -- so this is the function that *produces* a tenant,
    /// not one more call site that must already have one.
    ///
    /// Returns the whole `Customer` rather than a bare `TenantId` so the
    /// caller receives the tenant *attached to the row it was derived from*,
    /// with no opportunity to pair it with a different customer. Excludes
    /// soft-deleted rows, like every other read on this trait.
    fn find_by_stripe_customer_id(
        &self,
        stripe_customer_id: &str,
    ) -> impl Future<Output = Result<Option<Customer>, DomainError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_as_uuid() {
        let id = Uuid::new_v4();
        let customer_id = CustomerId::new(id);
        assert_eq!(customer_id.as_uuid(), id);
    }
}
