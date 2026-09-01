//! The read use cases behind the five `GET` routes (`init-spec.md` §8.2).
//!
//! Each is a thin pass-through to one repository today. They exist as their
//! own layer anyway because `api` depends on `service`, not on
//! `persistence`: a route reaching a repository directly would skip the
//! layer the architecture is built on. When 4c adds proration and payment
//! method ordering, they land in functions that already exist.

use async_trait::async_trait;
use domain::{
    DomainError, InvoiceRepository, PaymentMethodRepository, Plan, PlanRepository,
    SubscriptionRepository, TenantId,
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
}

/// Holds the four read repositories a host wires in.
///
/// Generic over each port, matching [`WebhookProcessor`]: `demo` monomorphises
/// the concrete `persistence` types, and nothing is boxed on the read path.
/// All four repositories are taken now even though only `plans` is read yet,
/// so `AppState` and every host's wiring stay fixed while the trait fills in
/// over Tasks 3, 4, 7 and 8.
///
/// [`WebhookProcessor`]: crate::WebhookProcessor
pub struct ReadService<L, S, I, M> {
    plans: L,
    // Wired now, read by later slices: subscriptions (Task 3), payment
    // methods (Task 4), invoices (Tasks 7-8).
    #[allow(dead_code)]
    subscriptions: S,
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
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use domain::{Currency, Money, Plan, PlanId};
    use time::OffsetDateTime;
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
    ) -> ReadService<InMemoryPlans, InMemorySubscriptions, InMemoryInvoices, InMemoryPaymentMethods>
    {
        ReadService::new(
            plans,
            InMemorySubscriptions::default(),
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

    #[tokio::test]
    async fn list_plans_returns_the_tenants_plans() -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let plans = InMemoryPlans::default();
        plans.seed(plan(tenant, "starter"));
        plans.seed(plan(tenant, "pro"));

        let names: Vec<_> = service(plans)
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

        let got = service(plans).list_plans(wanted).await?;

        assert_eq!(got.len(), 1);
        assert_eq!(got[0].tenant_id, wanted);
        Ok(())
    }
}
