use std::future::Future;

use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DomainError, Money, TenantId};

/// Identifies a `Plan`. Distinct from other entities' ids so the compiler
/// rejects passing the wrong id where a plan id is expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlanId(Uuid);

impl PlanId {
    /// Wraps a raw `Uuid` as a `PlanId`.
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the underlying `Uuid`.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

/// A purchasable price point, backed by a Stripe price and product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The plan's id.
    pub id: PlanId,
    /// The tenant this plan belongs to.
    pub tenant_id: TenantId,
    /// The linked Stripe price id.
    pub stripe_price_id: String,
    /// The linked Stripe product id.
    pub stripe_product_id: String,
    /// A human-readable name for the plan.
    pub name: String,
    /// The plan's price.
    pub amount: Money,
    /// When the plan was created.
    pub created_at: OffsetDateTime,
    /// When the plan was soft-deleted, if at all.
    pub deleted_at: Option<OffsetDateTime>,
}

/// Port for persisting and querying `Plan` records. Implemented by an
/// adapter crate (`persistence`); no I/O here.
///
/// Written as `fn … -> impl Future<Output = …> + Send` rather than bare
/// `async fn`, matching the other repositories the webhook path touches: the
/// `checkout.session.completed` handler resolves a local plan from a Stripe
/// price id inside `WebhookProcessor`, which is behind `#[async_trait]` and
/// boxes its awaited futures as `Send`. Implementors still write `async fn`.
pub trait PlanRepository {
    /// Creates a new plan for the given tenant.
    fn create(
        &self,
        tenant_id: TenantId,
        stripe_price_id: String,
        stripe_product_id: String,
        name: String,
        amount: Money,
    ) -> impl Future<Output = Result<Plan, DomainError>> + Send;

    /// Finds a plan by id, scoped to the tenant. Returns `None` if the plan
    /// does not exist or has been soft-deleted.
    fn find(
        &self,
        tenant_id: TenantId,
        id: PlanId,
    ) -> impl Future<Output = Result<Option<Plan>, DomainError>> + Send;

    /// Lists all plans for the given tenant, excluding soft-deleted ones.
    fn list(
        &self,
        tenant_id: TenantId,
    ) -> impl Future<Output = Result<Vec<Plan>, DomainError>> + Send;

    /// Finds a plan by its Stripe **price** id, scoped to the tenant. `None`
    /// if no local plan mirrors that price -- which the
    /// `checkout.session.completed` handler treats as
    /// `NotApplied(UnknownPlan)` rather than mirroring a subscription with a
    /// wrong `plan_id` (`billing.subscriptions.plan_id` is `NOT NULL`, so
    /// there is no honest way to mirror without a real plan row).
    fn find_by_stripe_price_id(
        &self,
        tenant_id: TenantId,
        stripe_price_id: &str,
    ) -> impl Future<Output = Result<Option<Plan>, DomainError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_as_uuid() {
        let id = Uuid::new_v4();
        let plan_id = PlanId::new(id);
        assert_eq!(plan_id.as_uuid(), id);
    }
}
