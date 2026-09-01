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
    BillingProvider, CancellationTiming, CreateCustomerParams, CustomerRepository, DomainError,
    PaymentMethodRepository, PlanId, PlanRepository, SetupIntentSnapshot, Subscription,
    SubscriptionId, SubscriptionRepository, SubscriptionSnapshot, SubscriptionStatus, TenantId,
};
use time::OffsetDateTime;

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

    /// Changes the tenant's subscription to `plan_id` (a **local** plan id,
    /// resolved to a Stripe price id here) and returns the subscription's
    /// current state afterward.
    ///
    /// Ownership is checked before Stripe is ever called: an unknown
    /// `subscription_id`, or one belonging to another tenant, is
    /// [`DomainError::NotFound`] with **no outbound call**; so is an unknown
    /// `plan_id`.
    ///
    /// On success the mirror is updated by **two** writes, deliberately
    /// separate because they answer to different authorities:
    ///
    /// - [`SubscriptionRepository::set_plan`] repoints `plan_id` at the plan
    ///   *this caller* named. Unguarded: no webhook ever writes that column,
    ///   so there is no event to be stale against.
    /// - The snapshot Stripe returned goes through
    ///   [`SubscriptionRepository::apply_event`]'s §10.2 ordering guard, so a
    ///   `customer.subscription.updated` webhook racing this call cannot be
    ///   regressed by it (or vice versa) -- this is the one place outside the
    ///   webhook path that writes those columns, and it borrows the webhook
    ///   path's own safety.
    ///
    /// A stale snapshot therefore leaves status and period bounds at the
    /// newer webhook's values while `plan_id` still moves, which is correct:
    /// the caller's plan choice is not something a webhook can be newer than.
    async fn change_plan(
        &self,
        tenant: TenantId,
        subscription_id: SubscriptionId,
        plan_id: PlanId,
    ) -> Result<Subscription, DomainError>;

    /// Cancels the tenant's subscription, at the period boundary
    /// (`at_period_end: true`) or immediately, and returns its current state
    /// afterward.
    ///
    /// Ownership is checked before Stripe is called, exactly as in
    /// [`change_plan`](Self::change_plan). A subscription that is **already**
    /// `Canceled` is returned as-is with no second Stripe call --
    /// cancellation is idempotent from the caller's point of view, and
    /// Stripe would reject an update to an already-terminated subscription
    /// anyway. Otherwise the returned snapshot is applied through
    /// `apply_event`, exactly as in `change_plan`.
    async fn cancel_subscription(
        &self,
        tenant: TenantId,
        subscription_id: SubscriptionId,
        at_period_end: bool,
    ) -> Result<Subscription, DomainError>;
}

/// Applies a direct (non-webhook) [`SubscriptionSnapshot`] through
/// [`SubscriptionRepository::apply_event`], then returns the row's current
/// state regardless of whether this particular write was admitted.
///
/// Shared by [`Writes::change_plan`] and [`Writes::cancel_subscription`]:
/// both call a provider method that returns a fresh `SubscriptionSnapshot`
/// and must apply it the same guarded way, and duplicating that dance
/// invites the two copies to quietly drift.
///
/// `event_created_at` is `OffsetDateTime::now_utc()` -- there is no Stripe
/// event here, only an API response, so "now" is this write's honest
/// timestamp for the ordering guard. Re-reading the row afterward (rather
/// than trusting the snapshot) is what makes a `Stale` result harmless: the
/// caller always sees whatever is currently authoritative, never a state the
/// guard just rejected.
async fn apply_subscription_snapshot<S: SubscriptionRepository + Send + Sync>(
    subscriptions: &S,
    tenant: TenantId,
    subscription_id: SubscriptionId,
    snapshot: SubscriptionSnapshot,
) -> Result<Subscription, DomainError> {
    // `EventApplication::Applied` vs `Stale` is deliberately not branched on
    // here -- the re-`find` below returns the row's current state either
    // way, which is exactly right for both outcomes.
    let _application = subscriptions
        .apply_event(
            tenant,
            subscription_id,
            snapshot.status,
            snapshot.current_period_start,
            snapshot.current_period_end,
            snapshot.cancel_at_period_end,
            OffsetDateTime::now_utc(),
        )
        .await?;
    subscriptions
        .find(tenant, subscription_id)
        .await?
        .ok_or(DomainError::NotFound)
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

    async fn change_plan(
        &self,
        tenant: TenantId,
        subscription_id: SubscriptionId,
        plan_id: PlanId,
    ) -> Result<Subscription, DomainError> {
        let subscription = self
            .subscriptions
            .find(tenant, subscription_id)
            .await?
            .ok_or(DomainError::NotFound)?;
        let plan = self
            .plans
            .find(tenant, plan_id)
            .await?
            .ok_or(DomainError::NotFound)?;

        let snapshot = self
            .provider
            .change_plan(
                tenant,
                &subscription.stripe_subscription_id,
                &subscription.stripe_subscription_item_id,
                &plan.stripe_price_id,
            )
            .await?;

        // Stripe accepted the change, so the mirror may now be repointed at
        // the new plan. Two separate writes because they answer to two
        // different authorities: `set_plan` records what *this caller* asked
        // for (unguarded -- no webhook writes `plan_id`), while the snapshot
        // below records what *Stripe* reported and goes through §10.2's
        // ordering guard. See `SubscriptionRepository::set_plan`'s rustdoc.
        self.subscriptions
            .set_plan(tenant, subscription_id, plan_id)
            .await?;

        apply_subscription_snapshot(&self.subscriptions, tenant, subscription_id, snapshot).await
    }

    async fn cancel_subscription(
        &self,
        tenant: TenantId,
        subscription_id: SubscriptionId,
        at_period_end: bool,
    ) -> Result<Subscription, DomainError> {
        let subscription = self
            .subscriptions
            .find(tenant, subscription_id)
            .await?
            .ok_or(DomainError::NotFound)?;

        if subscription.status == SubscriptionStatus::Canceled {
            return Ok(subscription);
        }

        let timing = if at_period_end {
            CancellationTiming::AtPeriodEnd
        } else {
            CancellationTiming::Immediate
        };
        let snapshot = self
            .provider
            .cancel_subscription(tenant, &subscription.stripe_subscription_id, timing)
            .await?;

        apply_subscription_snapshot(&self.subscriptions, tenant, subscription_id, snapshot).await
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use domain::{Currency, Customer, CustomerId, Money, Plan};
    use time::{Duration, OffsetDateTime};
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

    fn subscription(
        tenant: TenantId,
        status: SubscriptionStatus,
        last_event_created_at: Option<OffsetDateTime>,
    ) -> Subscription {
        let now = OffsetDateTime::now_utc();
        Subscription {
            id: SubscriptionId::new(Uuid::new_v4()),
            tenant_id: tenant,
            customer_id: CustomerId::new(Uuid::new_v4()),
            plan_id: PlanId::new(Uuid::new_v4()),
            stripe_subscription_id: format!("sub_{}", Uuid::new_v4()),
            stripe_subscription_item_id: "si_original".to_string(),
            status,
            current_period_start: now,
            current_period_end: now + Duration::days(30),
            cancel_at_period_end: false,
            last_event_created_at,
            created_at: now,
            deleted_at: None,
        }
    }

    fn plan(tenant: TenantId, stripe_price_id: &str) -> Plan {
        Plan {
            id: PlanId::new(Uuid::new_v4()),
            tenant_id: tenant,
            stripe_price_id: stripe_price_id.to_string(),
            stripe_product_id: "prod_test".to_string(),
            name: "test plan".to_string(),
            amount: Money::new(1999, Currency::Usd),
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

    #[tokio::test]
    async fn change_plan_resolves_the_price_and_applies_the_snapshot() -> Result<(), Box<dyn Error>>
    {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        let sub = subscription(tenant, SubscriptionStatus::PastDue, None);
        let sub_id = sub.id;
        let stripe_subscription_id = sub.stripe_subscription_id.clone();
        let stripe_item_id = sub.stripe_subscription_item_id.clone();
        svc.subscriptions.seed(sub);
        let new_plan = plan(tenant, "price_new");
        let new_plan_id = new_plan.id;
        svc.plans.seed(new_plan);

        let updated = svc.change_plan(tenant, sub_id, new_plan_id).await?;

        // The provider was called with the subscription's own Stripe ids and
        // the target plan's price -- never the local uuids.
        assert_eq!(
            svc.provider.change_plan_calls(),
            vec![(
                stripe_subscription_id,
                stripe_item_id,
                "price_new".to_string()
            )]
        );
        // The stub's snapshot (status Active) was applied through apply_event.
        assert_eq!(updated.status, SubscriptionStatus::Active);
        // ...and the mirror was repointed at the plan the caller asked for.
        // Without `set_plan` this silently kept the old plan forever, since
        // no webhook writes `plan_id` either.
        assert_eq!(updated.plan_id, new_plan_id);
        Ok(())
    }

    #[tokio::test]
    async fn change_plan_for_another_tenants_subscription_is_not_found()
    -> Result<(), Box<dyn Error>> {
        let mine = TenantId::new(Uuid::new_v4());
        let theirs = TenantId::new(Uuid::new_v4());
        let svc = service();
        let their_sub = subscription(theirs, SubscriptionStatus::Active, None);
        let their_sub_id = their_sub.id;
        svc.subscriptions.seed(their_sub);
        let their_plan = plan(theirs, "price_theirs");
        let their_plan_id = their_plan.id;
        svc.plans.seed(their_plan);

        let result = svc.change_plan(mine, their_sub_id, their_plan_id).await;

        assert!(matches!(result, Err(DomainError::NotFound)));
        assert!(svc.provider.change_plan_calls().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn change_plan_with_an_unknown_plan_id_is_not_found() -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        let sub = subscription(tenant, SubscriptionStatus::Active, None);
        let sub_id = sub.id;
        svc.subscriptions.seed(sub);

        let result = svc
            .change_plan(tenant, sub_id, PlanId::new(Uuid::new_v4()))
            .await;

        assert!(matches!(result, Err(DomainError::NotFound)));
        assert!(svc.provider.change_plan_calls().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn change_plan_does_not_regress_a_row_a_newer_event_already_applied()
    -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        // A newer event (an hour from now) has already been applied to this
        // row -- simulating a `customer.subscription.updated` webhook that
        // raced ahead of this call's own response.
        let future = OffsetDateTime::now_utc() + Duration::hours(1);
        let sub = subscription(tenant, SubscriptionStatus::PastDue, Some(future));
        let sub_id = sub.id;
        svc.subscriptions.seed(sub);
        let new_plan = plan(tenant, "price_new");
        let new_plan_id = new_plan.id;
        svc.plans.seed(new_plan);

        let returned = svc.change_plan(tenant, sub_id, new_plan_id).await?;

        // Stripe was still called (the plan change itself is not skipped),
        // but the guard rejected the write as stale, so the row -- and the
        // response -- keep the newer state rather than regressing to what
        // this call's own (now-stale) snapshot said.
        assert_eq!(svc.provider.change_plan_calls().len(), 1);
        assert_eq!(returned.status, SubscriptionStatus::PastDue);
        // `plan_id` still moves, though: it is written by `set_plan`, which
        // is deliberately unguarded because no webhook ever writes that
        // column, so there is no newer event for it to lose to.
        assert_eq!(returned.plan_id, new_plan_id);
        Ok(())
    }

    #[tokio::test]
    async fn cancel_at_period_end_maps_to_the_named_timing() -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        let sub = subscription(tenant, SubscriptionStatus::Active, None);
        let sub_id = sub.id;
        let stripe_subscription_id = sub.stripe_subscription_id.clone();
        svc.subscriptions.seed(sub);

        let updated = svc.cancel_subscription(tenant, sub_id, true).await?;

        assert_eq!(
            svc.provider.cancel_calls(),
            vec![(stripe_subscription_id, CancellationTiming::AtPeriodEnd)]
        );
        assert!(updated.cancel_at_period_end);
        Ok(())
    }

    #[tokio::test]
    async fn cancel_immediately_maps_to_the_named_timing() -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        let sub = subscription(tenant, SubscriptionStatus::Active, None);
        let sub_id = sub.id;
        let stripe_subscription_id = sub.stripe_subscription_id.clone();
        svc.subscriptions.seed(sub);

        let updated = svc.cancel_subscription(tenant, sub_id, false).await?;

        assert_eq!(
            svc.provider.cancel_calls(),
            vec![(stripe_subscription_id, CancellationTiming::Immediate)]
        );
        assert_eq!(updated.status, SubscriptionStatus::Canceled);
        Ok(())
    }

    #[tokio::test]
    async fn cancel_for_another_tenants_subscription_is_not_found() -> Result<(), Box<dyn Error>> {
        let mine = TenantId::new(Uuid::new_v4());
        let theirs = TenantId::new(Uuid::new_v4());
        let svc = service();
        let their_sub = subscription(theirs, SubscriptionStatus::Active, None);
        let their_sub_id = their_sub.id;
        svc.subscriptions.seed(their_sub);

        let result = svc.cancel_subscription(mine, their_sub_id, true).await;

        assert!(matches!(result, Err(DomainError::NotFound)));
        assert!(svc.provider.cancel_calls().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_an_already_canceled_subscription_is_a_no_op() -> Result<(), Box<dyn Error>>
    {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        let sub = subscription(tenant, SubscriptionStatus::Canceled, None);
        let sub_id = sub.id;
        svc.subscriptions.seed(sub);

        let returned = svc.cancel_subscription(tenant, sub_id, true).await?;

        assert_eq!(returned.status, SubscriptionStatus::Canceled);
        assert!(
            svc.provider.cancel_calls().is_empty(),
            "an already-canceled subscription must not place a second Stripe call"
        );
        Ok(())
    }
}
