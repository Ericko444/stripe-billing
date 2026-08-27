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
/// Uses native `async fn` rather than `async-trait` so `domain` doesn't need
/// that dependency; this trait is meant to be used generically
/// (`fn new<R: CustomerRepository>(repo: R)`), not as `dyn CustomerRepository`.
#[allow(async_fn_in_trait)]
pub trait CustomerRepository {
    /// Creates a new customer for the given tenant.
    async fn create(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: Option<String>,
    ) -> Result<Customer, DomainError>;

    /// Finds a customer by id, scoped to the tenant. Returns `None` if the
    /// customer does not exist or has been soft-deleted.
    async fn find(
        &self,
        tenant_id: TenantId,
        id: CustomerId,
    ) -> Result<Option<Customer>, DomainError>;

    /// Lists all customers for the given tenant, excluding soft-deleted ones.
    async fn list(&self, tenant_id: TenantId) -> Result<Vec<Customer>, DomainError>;
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
