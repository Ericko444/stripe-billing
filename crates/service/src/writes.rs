//! The write use cases behind Phase 4c's six mutating routes
//! (`docs/spec/phase-4c-write-routes.md`).
//!
//! The mirror of [`reads`](crate::reads): where [`Reads`](crate::Reads) is a
//! façade over the read repositories, [`Writes`] is the façade over the one
//! [`BillingProvider`] and the four repositories a mutating call touches. It
//! arrives with **no methods** and grows one per vertical slice -- the same
//! discipline `Reads` followed across Phase 4b.
//!
//! Kept a *second* port beside `Reads` rather than more methods on it (Plan
//! 4c, P1): a host test that needs a read stub should not also have to stub
//! six write methods it never calls, and the read path stays free of the
//! provider -- a [`ReadService`](crate::ReadService) still needs no Stripe
//! client, which is what keeps 4b's read tests running in a millisecond.

use async_trait::async_trait;
use domain::{
    BillingProvider, CreateCustomerParams, CustomerRepository, DomainError,
    PaymentMethodRepository, PlanRepository, SetupIntentSnapshot, SubscriptionRepository, TenantId,
};

/// Object-safe façade over the write use cases.
///
/// `api`'s `AppState` holds this as `Arc<dyn Writes>`, next to
/// `Arc<dyn Reads>` -- so `billing_router` still carries exactly one type
/// parameter and `AppState` gains exactly one field (Plan 4c, P1).
/// `#[async_trait]` for the same reason [`Reads`](crate::Reads) uses it: the
/// trait must be dyn-compatible.
///
/// Grows **one method per vertical slice** rather than arriving complete,
/// the same discipline [`Reads`](crate::Reads) followed -- that keeps it
/// honest about what is actually wired.
#[async_trait]
pub trait Writes: Send + Sync {
    /// Returns the tenant's Stripe customer id, creating the Stripe customer
    /// and the local `customers` row on first use.
    ///
    /// Routeless (`docs/spec/phase-4c-write-routes.md` §8.2): the callers are
    /// other `Writes` methods -- `start_checkout_session` and
    /// `create_setup_intent` -- that must name a customer to Stripe.
    ///
    /// A tenant counts as **linked** once it has a `customers` row carrying a
    /// `stripe_customer_id`; that id is returned and the provider is **not**
    /// called. `CustomerRepository` has no update, so a row with
    /// `stripe_customer_id = None` cannot be upgraded in place -- nothing
    /// creates such a row today (Plan 4c, Open Question 1), and changing that
    /// is a `domain` port change rather than a tweak here.
    async fn ensure_customer(&self, tenant: TenantId) -> Result<String, DomainError>;

    /// Creates a Stripe SetupIntent for the calling tenant, resolving (or
    /// creating) their Stripe customer via [`ensure_customer`](Self::ensure_customer)
    /// first -- a tenant with no customer yet still succeeds.
    ///
    /// The returned [`SetupIntentSnapshot`] carries only the browser-destined
    /// `client_secret`. The route puts it in the response body and **nowhere
    /// else** -- never a log line (`docs/spec/phase-4c-write-routes.md` §9).
    async fn create_setup_intent(
        &self,
        tenant: TenantId,
    ) -> Result<SetupIntentSnapshot, DomainError>;
}

/// Holds the [`BillingProvider`] and the four repositories a mutating call
/// touches.
///
/// Generic over each port, matching [`ReadService`](crate::ReadService) and
/// `WebhookProcessor`: `demo` monomorphises the concrete `stripe-adapter`
/// and `persistence` types, and nothing is boxed on the write path. The
/// `dyn` boundary is `Arc<dyn Writes>` on `AppState`, and nowhere else
/// (Plan 4c, P2).
///
/// All five dependencies are taken from the start, like `ReadService`'s
/// four, so `AppState` and every host's wiring stay fixed as the trait
/// fills in. `subscriptions`, `payment_methods` and `plans` stay unread
/// until the slices that need them (Tasks 8-16).
#[allow(dead_code)]
pub struct WriteService<P, C, S, M, L> {
    provider: P,
    customers: C,
    subscriptions: S,
    payment_methods: M,
    plans: L,
}

impl<P, C, S, M, L> WriteService<P, C, S, M, L> {
    /// Wraps the provider and the four write repositories.
    pub fn new(provider: P, customers: C, subscriptions: S, payment_methods: M, plans: L) -> Self {
        Self {
            provider,
            customers,
            subscriptions,
            payment_methods,
            plans,
        }
    }
}

#[async_trait]
impl<P, C, S, M, L> Writes for WriteService<P, C, S, M, L>
where
    P: BillingProvider,
    C: CustomerRepository + Send + Sync,
    S: SubscriptionRepository + Send + Sync,
    M: PaymentMethodRepository + Send + Sync,
    L: PlanRepository + Send + Sync,
{
    async fn ensure_customer(&self, tenant: TenantId) -> Result<String, DomainError> {
        // Already linked? Hand back that id and never touch Stripe. This is
        // the common path -- every checkout and setup-intent call after the
        // first.
        if let Some(id) = self
            .customers
            .list(tenant)
            .await?
            .into_iter()
            .find_map(|customer| customer.stripe_customer_id)
        {
            return Ok(id);
        }

        // Absent: Stripe is authoritative, so create there first, then
        // mirror. The id only exists once Stripe has minted it, so this
        // order is forced -- `CustomerRepository::create` needs it. A create
        // that succeeds at Stripe but fails to persist leaves an unlinked
        // Stripe customer; the ledger stopped a duplicate, and the webhook
        // path re-links it via `find_by_stripe_customer_id`.
        let snapshot = self
            .provider
            .create_customer(
                tenant,
                CreateCustomerParams {
                    email: None,
                    name: None,
                },
            )
            .await?;
        self.customers
            .create(tenant, Some(snapshot.stripe_customer_id.clone()))
            .await?;
        Ok(snapshot.stripe_customer_id)
    }

    async fn create_setup_intent(
        &self,
        tenant: TenantId,
    ) -> Result<SetupIntentSnapshot, DomainError> {
        let stripe_customer_id = self.ensure_customer(tenant).await?;
        self.provider
            .create_setup_intent(tenant, &stripe_customer_id)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use domain::{Customer, CustomerId};
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{
        InMemoryCustomers, InMemoryPaymentMethods, InMemoryPlans, InMemorySubscriptions,
        StubBillingProvider,
    };

    type TestWrites = WriteService<
        StubBillingProvider,
        InMemoryCustomers,
        InMemorySubscriptions,
        InMemoryPaymentMethods,
        InMemoryPlans,
    >;

    /// Compiles only if `Writes` is dyn-compatible -- the property the
    /// `#[async_trait]` decision exists for.
    #[allow(dead_code)]
    fn assert_dyn_compatible(_w: &dyn Writes) {}

    /// A `WriteService` over the in-memory doubles. Seed and inspect through
    /// the fields: `svc.customers.seed(..)`, `svc.provider.create_customer_calls()`.
    fn service() -> TestWrites {
        WriteService::new(
            StubBillingProvider::default(),
            InMemoryCustomers::default(),
            InMemorySubscriptions::default(),
            InMemoryPaymentMethods::default(),
            InMemoryPlans::default(),
        )
    }

    fn linked_customer(tenant: TenantId, stripe_customer_id: &str) -> Customer {
        Customer {
            id: CustomerId::new(Uuid::new_v4()),
            tenant_id: tenant,
            stripe_customer_id: Some(stripe_customer_id.to_string()),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        }
    }

    #[tokio::test]
    async fn returns_the_existing_id_without_calling_the_provider() -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        svc.customers.seed(linked_customer(tenant, "cus_existing"));

        let id = svc.ensure_customer(tenant).await?;

        assert_eq!(id, "cus_existing");
        assert_eq!(svc.provider.create_customer_calls(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn creates_at_stripe_then_persists_locally_when_absent() -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();

        let id = svc.ensure_customer(tenant).await?;

        assert_eq!(svc.provider.create_customer_calls(), 1);
        // The local row now exists and carries the same id.
        let rows = svc.customers.list(tenant).await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].stripe_customer_id.as_deref(), Some(id.as_str()));

        // A second call is the "already linked" path: no further provider call.
        let again = svc.ensure_customer(tenant).await?;
        assert_eq!(again, id);
        assert_eq!(svc.provider.create_customer_calls(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn create_setup_intent_resolves_the_customer_first() -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();

        let snapshot = svc.create_setup_intent(tenant).await?;

        // ensure_customer created a customer (provider hit once), and the
        // SetupIntent was for that same id.
        assert_eq!(svc.provider.create_customer_calls(), 1);
        let linked = svc.customers.list(tenant).await?[0]
            .stripe_customer_id
            .clone()
            .ok_or("the customer should be linked after ensure_customer")?;
        assert_eq!(svc.provider.setup_intent_customers(), vec![linked.clone()]);
        assert!(snapshot.client_secret.contains(&linked));
        assert!(snapshot.client_secret.contains("_secret_"));
        Ok(())
    }

    #[tokio::test]
    async fn create_setup_intent_reuses_a_linked_customer() -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        svc.customers.seed(linked_customer(tenant, "cus_linked"));

        svc.create_setup_intent(tenant).await?;

        assert_eq!(svc.provider.create_customer_calls(), 0);
        assert_eq!(
            svc.provider.setup_intent_customers(),
            vec!["cus_linked".to_string()]
        );
        Ok(())
    }

    #[tokio::test]
    async fn another_tenants_customer_is_never_returned() -> Result<(), Box<dyn Error>> {
        let mine = TenantId::new(Uuid::new_v4());
        let theirs = TenantId::new(Uuid::new_v4());
        let svc = service();
        svc.customers.seed(linked_customer(theirs, "cus_theirs"));

        let id = svc.ensure_customer(mine).await?;

        // Their linked id was neither returned nor seen; mine was created fresh.
        assert_ne!(id, "cus_theirs");
        assert_eq!(svc.provider.create_customer_calls(), 1);
        let mine_rows = svc.customers.list(mine).await?;
        assert_eq!(mine_rows.len(), 1);
        assert_eq!(mine_rows[0].tenant_id, mine);
        Ok(())
    }
}
