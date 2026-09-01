//! The read use cases behind the five `GET` routes (`init-spec.md` §8.2).
//!
//! Each is a thin pass-through to one repository today. They exist as their
//! own layer anyway because `api` depends on `service`, not on
//! `persistence`: a route reaching a repository directly would skip the
//! layer the architecture is built on. When 4c adds proration and payment
//! method ordering, they land in functions that already exist.

use async_trait::async_trait;
use domain::{
    DomainError, InvoiceRepository, PaymentMethodRepository, Plan, PlanRepository, Subscription,
    SubscriptionRepository, SubscriptionStatus, TenantId,
};

/// Object-safe façade over the read use cases.
///
/// `api`'s `AppState` holds this as `Arc<dyn Reads>`, never the generic
/// [`ReadService`] directly -- the same trade [`WebhookHandler`] made, so the
/// router factory does not carry four repository type parameters through its
/// signature.
///
/// The trait grows **one method per vertical slice** rather than arriving
/// complete: that keeps it honest about what is actually wired.
///
/// [`WebhookHandler`]: crate::WebhookHandler
#[async_trait]
pub trait Reads: Send + Sync {
    /// Every plan for the tenant, soft-deleted rows excluded (the repository
    /// already does the exclusion).
    async fn list_plans(&self, tenant: TenantId) -> Result<Vec<Plan>, DomainError>;

    /// The tenant's current subscription, or `None` if they have never
    /// subscribed -- a normal state, not an error and not a 404 at the route.
    ///
    /// "Current" is the most recently created subscription that is **not**
    /// `Canceled`. `billing.subscriptions` has no "one active per tenant"
    /// constraint and the repository returns every row; enforcing that
    /// invariant is a §7 concern this phase does not take on (Open
    /// Question 2).
    async fn get_current_subscription(
        &self,
        tenant: TenantId,
    ) -> Result<Option<Subscription>, DomainError>;
}

/// Holds the four read repositories a host wires in.
///
/// Generic over each port, matching [`WebhookProcessor`]: `demo` monomorphises
/// the concrete `persistence` types, and nothing is boxed on the read path.
/// All four repositories are taken from the start, so `AppState` and every
/// host's wiring stay fixed while the trait fills in over Tasks 4, 7 and 8.
///
/// [`WebhookProcessor`]: crate::WebhookProcessor
pub struct ReadService<L, S, I, M> {
    plans: L,
    subscriptions: S,
    // Wired now, read by later slices: payment methods (Task 4), invoices
    // (Tasks 7-8).
    #[allow(dead_code)]
    invoices: I,
    #[allow(dead_code)]
    payment_methods: M,
}

impl<L, S, I, M> ReadService<L, S, I, M> {
    /// Wraps the read repositories.
    pub fn new(plans: L, subscriptions: S, invoices: I, payment_methods: M) -> Self {
        Self {
            plans,
            subscriptions,
            invoices,
            payment_methods,
        }
    }
}

#[async_trait]
impl<L, S, I, M> Reads for ReadService<L, S, I, M>
where
    L: PlanRepository + Send + Sync,
    S: SubscriptionRepository + Send + Sync,
    I: InvoiceRepository + Send + Sync,
    M: PaymentMethodRepository + Send + Sync,
{
    async fn list_plans(&self, tenant: TenantId) -> Result<Vec<Plan>, DomainError> {
        self.plans.list(tenant).await
    }

    async fn get_current_subscription(
        &self,
        tenant: TenantId,
    ) -> Result<Option<Subscription>, DomainError> {
        let mut subscriptions = self.subscriptions.list(tenant).await?;
        subscriptions.retain(|s| s.status != SubscriptionStatus::Canceled);
        subscriptions.sort_by_key(|s| s.created_at);
        Ok(subscriptions.pop())
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use domain::{Currency, CustomerId, Money, Plan, PlanId, SubscriptionId};
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{
        InMemoryInvoices, InMemoryPaymentMethods, InMemoryPlans, InMemorySubscriptions,
    };

    /// Compiles only if `Reads` is dyn-compatible -- the property the
    /// `#[async_trait]` decision exists for.
    #[allow(dead_code)]
    fn assert_dyn_compatible(_reads: &dyn Reads) {}

    fn service(
        plans: InMemoryPlans,
        subscriptions: InMemorySubscriptions,
    ) -> ReadService<InMemoryPlans, InMemorySubscriptions, InMemoryInvoices, InMemoryPaymentMethods>
    {
        ReadService::new(
            plans,
            subscriptions,
            InMemoryInvoices::default(),
            InMemoryPaymentMethods::default(),
        )
    }

    fn plan(tenant: TenantId, name: &str) -> Plan {
        Plan {
            id: PlanId::new(Uuid::new_v4()),
            tenant_id: tenant,
            stripe_price_id: format!("price_{name}"),
            stripe_product_id: format!("prod_{name}"),
            name: name.to_string(),
            amount: Money::new(1999, Currency::Eur),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        }
    }

    fn subscription(
        tenant: TenantId,
        status: SubscriptionStatus,
        created_at: OffsetDateTime,
    ) -> Subscription {
        Subscription {
            id: SubscriptionId::new(Uuid::new_v4()),
            tenant_id: tenant,
            customer_id: CustomerId::new(Uuid::new_v4()),
            plan_id: PlanId::new(Uuid::new_v4()),
            stripe_subscription_id: format!("sub_{}", Uuid::new_v4()),
            stripe_subscription_item_id: "si_test".to_string(),
            status,
            current_period_start: created_at,
            current_period_end: created_at + Duration::days(30),
            cancel_at_period_end: false,
            last_event_created_at: None,
            created_at,
            deleted_at: None,
        }
    }

    #[tokio::test]
    async fn list_plans_returns_the_tenants_plans() -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let plans = InMemoryPlans::default();
        plans.seed(plan(tenant, "starter"));
        plans.seed(plan(tenant, "pro"));

        let names: Vec<_> = service(plans, InMemorySubscriptions::default())
            .list_plans(tenant)
            .await?
            .into_iter()
            .map(|p| p.name)
            .collect();

        assert_eq!(names, ["starter", "pro"]);
        Ok(())
    }

    #[tokio::test]
    async fn list_plans_passes_the_tenant_through_unchanged() -> Result<(), Box<dyn Error>> {
        let wanted = TenantId::new(Uuid::new_v4());
        let other = TenantId::new(Uuid::new_v4());
        let plans = InMemoryPlans::default();
        plans.seed(plan(wanted, "mine"));
        plans.seed(plan(other, "theirs"));

        let got = service(plans, InMemorySubscriptions::default())
            .list_plans(wanted)
            .await?;

        assert_eq!(got.len(), 1);
        assert_eq!(got[0].tenant_id, wanted);
        Ok(())
    }

    #[tokio::test]
    async fn current_subscription_is_none_when_the_tenant_never_subscribed()
    -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());

        let got = service(InMemoryPlans::default(), InMemorySubscriptions::default())
            .get_current_subscription(tenant)
            .await?;

        assert!(got.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn current_subscription_is_the_newest_non_canceled_one() -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let epoch = OffsetDateTime::UNIX_EPOCH;
        let subscriptions = InMemorySubscriptions::default();
        // Newest overall, but canceled -- must be skipped.
        subscriptions.seed(subscription(
            tenant,
            SubscriptionStatus::Canceled,
            epoch + Duration::days(10),
        ));
        let newest_active = subscription(
            tenant,
            SubscriptionStatus::Active,
            epoch + Duration::days(5),
        );
        let newest_active_id = newest_active.id;
        subscriptions.seed(newest_active);
        subscriptions.seed(subscription(
            tenant,
            SubscriptionStatus::PastDue,
            epoch + Duration::days(1),
        ));

        let got = service(InMemoryPlans::default(), subscriptions)
            .get_current_subscription(tenant)
            .await?
            .ok_or("expected a current subscription")?;

        assert_eq!(got.id, newest_active_id);
        Ok(())
    }

    #[tokio::test]
    async fn current_subscription_is_tenant_scoped() -> Result<(), Box<dyn Error>> {
        let mine = TenantId::new(Uuid::new_v4());
        let theirs = TenantId::new(Uuid::new_v4());
        let subscriptions = InMemorySubscriptions::default();
        subscriptions.seed(subscription(
            theirs,
            SubscriptionStatus::Active,
            OffsetDateTime::UNIX_EPOCH,
        ));

        let got = service(InMemoryPlans::default(), subscriptions)
            .get_current_subscription(mine)
            .await?;

        assert!(got.is_none());
        Ok(())
    }
}
