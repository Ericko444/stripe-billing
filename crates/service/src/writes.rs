//! The write use cases behind the six mutating routes.
//!
//! The mirror of [`reads`](crate::reads): where [`Reads`](crate::Reads) is a
//! façade over the read repositories, [`Writes`] is the façade over the one
//! [`BillingProvider`] and the four repositories a mutating call touches. It
//! grew one method per vertical slice -- the same discipline `Reads`
//! followed.
//!
//! Kept a *second* port beside `Reads` rather than more methods on it: a
//! host test that needs a read stub should not also have to stub six write
//! methods it never calls, and the read path stays free of the provider --
//! a [`ReadService`](crate::ReadService) still needs no Stripe client, which
//! is what keeps the read tests running in a millisecond.

use async_trait::async_trait;
use audit::{Action, Actor, AuditEntry, CorrelationId, Target, TargetId};
use domain::{
    BillingProvider, CancellationTiming, CheckoutSessionParams, CheckoutSessionSnapshot,
    CreateCustomerParams, CustomerRepository, DomainError, PaymentMethod, PaymentMethodId,
    PaymentMethodRepository, PlanId, PlanRepository, SetupIntentSnapshot, Subscription,
    SubscriptionId, SubscriptionRepository, SubscriptionStatus, TenantId,
};
use time::OffsetDateTime;

use crate::RequestContext;

/// Object-safe façade over the write use cases.
///
/// `api`'s `AppState` holds this as `Arc<dyn Writes>`, next to
/// `Arc<dyn Reads>` -- so `billing_router` still carries exactly one type
/// parameter, the host's tenant extractor.
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
    /// Routeless: the callers are
    /// other `Writes` methods -- `start_checkout_session` and
    /// `create_setup_intent` -- that must name a customer to Stripe.
    ///
    /// A tenant counts as **linked** once it has a `customers` row carrying a
    /// `stripe_customer_id`; that id is returned and the provider is **not**
    /// called. `CustomerRepository` has no update, so a row with
    /// `stripe_customer_id = None` cannot be upgraded in place -- nothing
    /// creates such a row today, and changing that
    /// is a `domain` port change rather than a tweak here.
    async fn ensure_customer(&self, tenant: TenantId) -> Result<String, DomainError>;

    /// Creates a Stripe SetupIntent for the calling tenant, resolving (or
    /// creating) their Stripe customer via [`ensure_customer`](Self::ensure_customer)
    /// first -- a tenant with no customer yet still succeeds.
    ///
    /// The returned [`SetupIntentSnapshot`] carries only the browser-destined
    /// `client_secret`. The route puts it in the response body and **nowhere
    /// else** -- never a log line: whoever holds it can attach a payment
    /// method to the customer.
    async fn create_setup_intent(
        &self,
        ctx: RequestContext,
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
    /// On success the mirror is updated by
    /// [`SubscriptionRepository::change_plan`], one transaction covering
    /// both the plan repoint and the Stripe-reported snapshot -- merged
    /// from two separate writes specifically so an audit entry would have
    /// one honest write to be atomic with (see that method's own rustdoc).
    /// The *authority* split is unchanged, only the transaction boundary is:
    ///
    /// - The plan repoint answers only to *this caller*. Unguarded: no
    ///   webhook ever writes that column, so there is no event to be stale
    ///   against.
    /// - The snapshot Stripe returned goes through the same ordering guard
    ///   `apply_event` uses elsewhere, so a `customer.subscription.updated`
    ///   webhook racing this call cannot be regressed by it (or vice versa).
    ///
    /// A stale snapshot therefore leaves status and period bounds at the
    /// newer webhook's values while `plan_id` still moves, which is correct:
    /// the caller's plan choice is not something a webhook can be newer than.
    async fn change_plan(
        &self,
        ctx: RequestContext,
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
        ctx: RequestContext,
        subscription_id: SubscriptionId,
        at_period_end: bool,
    ) -> Result<Subscription, DomainError>;

    /// Makes `payment_method_id` the calling tenant's default, and returns
    /// the updated row.
    ///
    /// Ownership is checked first: an unknown or another tenant's payment
    /// method id is [`DomainError::NotFound`] with **no outbound call**.
    ///
    /// **Ordering: Stripe first, mirror second.** Stripe's
    /// `invoice_settings.default_payment_method` update runs before
    /// [`PaymentMethodRepository::set_default`] touches the local
    /// `is_default` flags, so a Stripe failure leaves those flags exactly as
    /// they were. The mirror reconciliation is a single statement -- old
    /// default cleared and new one set together, never two and never none.
    async fn set_default_payment_method(
        &self,
        ctx: RequestContext,
        payment_method_id: PaymentMethodId,
    ) -> Result<PaymentMethod, DomainError>;

    /// Removes `payment_method_id` -- detached at Stripe, then soft-deleted
    /// locally.
    ///
    /// Ownership is checked first: an unknown or another tenant's id is
    /// [`DomainError::NotFound`] with **no outbound call**.
    ///
    /// **Ordering: Stripe first, mirror second.** The detach runs
    /// before [`PaymentMethodRepository::detach_event`] sets `deleted_at`,
    /// so a failed detach leaves the row present -- never a mirror that
    /// claims the card is gone while Stripe still has it attached and
    /// billable. If a `payment_method.detached` webhook already soft-deleted
    /// the row, `detach_event` reports it stale; that is **not** an error
    /// here -- the card is gone either way.
    async fn remove_payment_method(
        &self,
        ctx: RequestContext,
        payment_method_id: PaymentMethodId,
    ) -> Result<(), DomainError>;

    /// Starts a `subscription`-mode Checkout Session for `plan_id` (a
    /// **local** plan id) and returns its hosted-page `url`.
    ///
    /// Resolves the local plan **first**: an unknown or another tenant's
    /// `plan_id` is [`DomainError::NotFound`] with **no outbound call**.
    /// Then [`ensure_customer`](Self::ensure_customer) (creating the Stripe
    /// customer on first use), then the session. `success_url` /
    /// `cancel_url` come from host config, threaded in by the route -- never
    /// a request field.
    ///
    /// No mirror write: the local `subscriptions` row is created by the
    /// `customer.subscription.created` webhook once the customer completes
    /// checkout. The returned `url` is browser-destined and must not be
    /// logged.
    async fn start_checkout_session(
        &self,
        ctx: RequestContext,
        plan_id: PlanId,
        success_url: &str,
        cancel_url: &str,
    ) -> Result<CheckoutSessionSnapshot, DomainError>;
}

/// Holds the [`BillingProvider`] and the four repositories a mutating call
/// touches.
///
/// Generic over each port, matching [`ReadService`](crate::ReadService) and
/// `WebhookProcessor`: `demo` monomorphises the concrete `stripe-adapter`
/// and `persistence` types, and nothing is boxed on the write path. The
/// `dyn` boundary is `Arc<dyn Writes>` on `AppState`, and nowhere else.
///
/// All five dependencies were taken from the start, like `ReadService`'s
/// four, so `AppState` and every host's wiring stayed fixed as the trait
/// filled in.
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
        ctx: RequestContext,
    ) -> Result<SetupIntentSnapshot, DomainError> {
        let tenant = ctx.tenant_id;
        let stripe_customer_id = self.ensure_customer(tenant).await?;
        self.provider
            .create_setup_intent(tenant, &stripe_customer_id)
            .await
    }

    async fn change_plan(
        &self,
        ctx: RequestContext,
        subscription_id: SubscriptionId,
        plan_id: PlanId,
    ) -> Result<Subscription, DomainError> {
        let tenant = ctx.tenant_id;
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
        // the new plan. One transaction, one port method
        // (`SubscriptionRepository::change_plan`, not the standalone
        // `set_plan` + `apply_event` this used to be) -- see that method's
        // rustdoc for why the two writes needed merging before this could be
        // audited honestly (F5). The *authority* split is unchanged: the
        // plan repoint still answers only to this caller, the snapshot still
        // goes through the webhook ordering guard.
        let now = OffsetDateTime::now_utc();
        let entry = AuditEntry::new(
            audit::TenantId::new(tenant.as_uuid()),
            Actor::System,
            Action::SubscriptionPlanChanged,
            Target::Subscription(TargetId::new(subscription_id.as_uuid())),
            now,
            CorrelationId::new(ctx.correlation_id),
        );
        self.subscriptions
            .change_plan(tenant, subscription_id, plan_id, snapshot, now, entry)
            .await
    }

    async fn cancel_subscription(
        &self,
        ctx: RequestContext,
        subscription_id: SubscriptionId,
        at_period_end: bool,
    ) -> Result<Subscription, DomainError> {
        let tenant = ctx.tenant_id;
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

        let now = OffsetDateTime::now_utc();
        let entry = AuditEntry::new(
            audit::TenantId::new(tenant.as_uuid()),
            Actor::System,
            Action::SubscriptionCanceled,
            Target::Subscription(TargetId::new(subscription_id.as_uuid())),
            now,
            CorrelationId::new(ctx.correlation_id),
        );
        self.subscriptions
            .cancel(tenant, subscription_id, snapshot, now, entry)
            .await
    }

    async fn set_default_payment_method(
        &self,
        ctx: RequestContext,
        payment_method_id: PaymentMethodId,
    ) -> Result<PaymentMethod, DomainError> {
        let tenant = ctx.tenant_id;
        let payment_method = self
            .payment_methods
            .find(tenant, payment_method_id)
            .await?
            .ok_or(DomainError::NotFound)?;

        // The *Stripe* customer id lives on the `Customer` row, not on the
        // payment method. The FK guarantees this lookup finds a row.
        let stripe_customer_id = self
            .customers
            .find(tenant, payment_method.customer_id)
            .await?
            .and_then(|customer| customer.stripe_customer_id)
            .ok_or_else(|| {
                DomainError::Repository(
                    "payment method's customer has no linked Stripe customer".to_string(),
                )
            })?;

        // Stripe first. `?` here returns before the mirror is touched,
        // so the `is_default` flags stay exactly as they were.
        self.provider
            .set_default_payment_method(
                tenant,
                &stripe_customer_id,
                &payment_method.stripe_payment_method_id,
            )
            .await?;

        // Then the mirror, in one guarded-free statement (old default
        // cleared and new one set together), audited in the same
        // transaction (D1(h)).
        let entry = AuditEntry::new(
            audit::TenantId::new(tenant.as_uuid()),
            Actor::System,
            Action::PaymentMethodSetDefault,
            Target::PaymentMethod(TargetId::new(payment_method_id.as_uuid())),
            OffsetDateTime::now_utc(),
            CorrelationId::new(ctx.correlation_id),
        );
        self.payment_methods
            .set_default(tenant, payment_method.customer_id, payment_method_id, entry)
            .await?;

        self.payment_methods
            .find(tenant, payment_method_id)
            .await?
            .ok_or(DomainError::NotFound)
    }

    async fn remove_payment_method(
        &self,
        ctx: RequestContext,
        payment_method_id: PaymentMethodId,
    ) -> Result<(), DomainError> {
        let tenant = ctx.tenant_id;
        let payment_method = self
            .payment_methods
            .find(tenant, payment_method_id)
            .await?
            .ok_or(DomainError::NotFound)?;

        // Stripe first. `?` returns before `detach_event`, so a failed
        // detach leaves the row present rather than a mirror that lies about
        // the card being gone.
        self.provider
            .detach_payment_method(tenant, &payment_method.stripe_payment_method_id)
            .await?;

        // Then the mirror, audited in the same transaction (D1(h)). `Applied`
        // vs `Stale` is not branched on: a stale result means a
        // `payment_method.detached` webhook already soft-deleted the row,
        // which is not an error -- the card is gone either way, and the
        // entry is written regardless (Stripe already confirmed the detach
        // above, so the request happened either way -- see
        // `PaymentMethodRepository::remove`'s own docs).
        let now = OffsetDateTime::now_utc();
        let entry = AuditEntry::new(
            audit::TenantId::new(tenant.as_uuid()),
            Actor::System,
            Action::PaymentMethodDetached,
            Target::PaymentMethod(TargetId::new(payment_method_id.as_uuid())),
            now,
            CorrelationId::new(ctx.correlation_id),
        );
        let _application = self
            .payment_methods
            .remove(tenant, &payment_method.stripe_payment_method_id, now, entry)
            .await?;

        Ok(())
    }

    async fn start_checkout_session(
        &self,
        ctx: RequestContext,
        plan_id: PlanId,
        success_url: &str,
        cancel_url: &str,
    ) -> Result<CheckoutSessionSnapshot, DomainError> {
        // Resolve the plan first: an unknown or cross-tenant id 404s here,
        // before `ensure_customer` could create a Stripe customer and before
        // the provider is reached.
        let tenant = ctx.tenant_id;
        let plan = self
            .plans
            .find(tenant, plan_id)
            .await?
            .ok_or(DomainError::NotFound)?;

        let stripe_customer_id = self.ensure_customer(tenant).await?;

        self.provider
            .create_checkout_session(
                tenant,
                CheckoutSessionParams {
                    stripe_customer_id,
                    stripe_price_id: plan.stripe_price_id,
                    success_url: success_url.to_string(),
                    cancel_url: cancel_url.to_string(),
                },
            )
            .await
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
        CallLog, InMemoryCustomers, InMemoryPaymentMethods, InMemoryPlans, InMemorySubscriptions,
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

    /// Wraps `tenant` in a `RequestContext` with a fresh correlation id --
    /// every test below cares about the tenant only, so this keeps the
    /// `RequestContext` plumbing out of each call site.
    fn ctx(tenant: TenantId) -> RequestContext {
        RequestContext::new(tenant, Uuid::new_v4())
    }

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

    fn payment_method(
        tenant: TenantId,
        customer_id: CustomerId,
        is_default: bool,
    ) -> PaymentMethod {
        PaymentMethod {
            id: PaymentMethodId::new(Uuid::new_v4()),
            tenant_id: tenant,
            customer_id,
            stripe_payment_method_id: format!("pm_{}", Uuid::new_v4()),
            brand: "visa".to_string(),
            last4: "4242".to_string(),
            is_default,
            last_event_created_at: None,
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        }
    }

    /// A `WriteService` whose provider double and payment-method double both
    /// push into `log` (`"provider"` / `"repository"`), and whose provider's
    /// payment-method calls fail when `fail_provider` -- the two knobs the
    /// Stripe-first ordering tests need.
    fn service_for_ordering(log: CallLog, fail_provider: bool) -> TestWrites {
        WriteService::new(
            StubBillingProvider::for_ordering_test(log.clone(), fail_provider),
            InMemoryCustomers::default(),
            InMemorySubscriptions::default(),
            InMemoryPaymentMethods::with_call_log(log),
            InMemoryPlans::default(),
        )
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

        let snapshot = svc.create_setup_intent(ctx(tenant)).await?;

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

        svc.create_setup_intent(ctx(tenant)).await?;

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

        let updated = svc.change_plan(ctx(tenant), sub_id, new_plan_id).await?;

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
    async fn change_plan_builds_an_audit_entry_naming_the_tenant_target_and_correlation_id()
    -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        let sub = subscription(tenant, SubscriptionStatus::Active, None);
        let sub_id = sub.id;
        svc.subscriptions.seed(sub);
        let new_plan = plan(tenant, "price_audit");
        let new_plan_id = new_plan.id;
        svc.plans.seed(new_plan);
        let correlation_id = Uuid::new_v4();

        svc.change_plan(
            RequestContext::new(tenant, correlation_id),
            sub_id,
            new_plan_id,
        )
        .await?;

        let entries = svc.subscriptions.change_plan_entries();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.tenant_id().as_uuid(), tenant.as_uuid());
        assert_eq!(entry.actor(), audit::Actor::System);
        assert_eq!(entry.action(), audit::Action::SubscriptionPlanChanged);
        assert_eq!(
            entry.target(),
            audit::Target::Subscription(audit::TargetId::new(sub_id.as_uuid()))
        );
        assert_eq!(entry.correlation_id().as_uuid(), correlation_id);
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

        let result = svc
            .change_plan(ctx(mine), their_sub_id, their_plan_id)
            .await;

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
            .change_plan(ctx(tenant), sub_id, PlanId::new(Uuid::new_v4()))
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

        let returned = svc.change_plan(ctx(tenant), sub_id, new_plan_id).await?;

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

        let updated = svc.cancel_subscription(ctx(tenant), sub_id, true).await?;

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

        let updated = svc.cancel_subscription(ctx(tenant), sub_id, false).await?;

        assert_eq!(
            svc.provider.cancel_calls(),
            vec![(stripe_subscription_id, CancellationTiming::Immediate)]
        );
        assert_eq!(updated.status, SubscriptionStatus::Canceled);
        Ok(())
    }

    #[tokio::test]
    async fn cancel_builds_an_audit_entry_naming_the_tenant_target_and_correlation_id()
    -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        let sub = subscription(tenant, SubscriptionStatus::Active, None);
        let sub_id = sub.id;
        svc.subscriptions.seed(sub);
        let correlation_id = Uuid::new_v4();

        svc.cancel_subscription(RequestContext::new(tenant, correlation_id), sub_id, true)
            .await?;

        let entries = svc.subscriptions.cancel_entries();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.tenant_id().as_uuid(), tenant.as_uuid());
        assert_eq!(entry.actor(), audit::Actor::System);
        assert_eq!(entry.action(), audit::Action::SubscriptionCanceled);
        assert_eq!(
            entry.target(),
            audit::Target::Subscription(audit::TargetId::new(sub_id.as_uuid()))
        );
        assert_eq!(entry.correlation_id().as_uuid(), correlation_id);
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

        let result = svc.cancel_subscription(ctx(mine), their_sub_id, true).await;

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

        let returned = svc.cancel_subscription(ctx(tenant), sub_id, true).await?;

        assert_eq!(returned.status, SubscriptionStatus::Canceled);
        assert!(
            svc.provider.cancel_calls().is_empty(),
            "an already-canceled subscription must not place a second Stripe call"
        );
        Ok(())
    }

    // --- set_default_payment_method (Stripe-first ordering) --------------

    #[tokio::test]
    async fn set_default_moves_the_flag_and_calls_stripe_for_the_customer()
    -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        let customer = linked_customer(tenant, "cus_pm");
        let customer_id = customer.id;
        svc.customers.seed(customer);
        let old_default = payment_method(tenant, customer_id, true);
        let new_default = payment_method(tenant, customer_id, false);
        let new_id = new_default.id;
        let new_stripe_id = new_default.stripe_payment_method_id.clone();
        let old_id = old_default.id;
        svc.payment_methods.seed(old_default);
        svc.payment_methods.seed(new_default);

        let returned = svc.set_default_payment_method(ctx(tenant), new_id).await?;

        assert!(returned.is_default);
        assert_eq!(returned.id, new_id);
        // The old default was cleared in the same reconciliation.
        let rows = svc.payment_methods.list(tenant).await?;
        let old = rows.iter().find(|p| p.id == old_id).ok_or("old row")?;
        assert!(!old.is_default, "the previous default must be cleared");
        // Stripe was told, keyed by the customer, not the local id.
        assert_eq!(
            svc.provider.set_default_calls(),
            vec![("cus_pm".to_string(), new_stripe_id)]
        );
        Ok(())
    }

    #[tokio::test]
    async fn set_default_builds_an_audit_entry_naming_the_tenant_target_and_correlation_id()
    -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        let customer = linked_customer(tenant, "cus_pm_audit");
        let customer_id = customer.id;
        svc.customers.seed(customer);
        let pm = payment_method(tenant, customer_id, false);
        let pm_id = pm.id;
        svc.payment_methods.seed(pm);
        let correlation_id = Uuid::new_v4();

        svc.set_default_payment_method(RequestContext::new(tenant, correlation_id), pm_id)
            .await?;

        let entries = svc.payment_methods.set_default_entries();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.tenant_id().as_uuid(), tenant.as_uuid());
        assert_eq!(entry.actor(), audit::Actor::System);
        assert_eq!(entry.action(), audit::Action::PaymentMethodSetDefault);
        assert_eq!(
            entry.target(),
            audit::Target::PaymentMethod(audit::TargetId::new(pm_id.as_uuid()))
        );
        assert_eq!(entry.correlation_id().as_uuid(), correlation_id);
        Ok(())
    }

    #[tokio::test]
    async fn set_default_calls_the_provider_before_the_repository() -> Result<(), Box<dyn Error>> {
        let log: CallLog = CallLog::default();
        let svc = service_for_ordering(log.clone(), false);
        let tenant = TenantId::new(Uuid::new_v4());
        let customer = linked_customer(tenant, "cus_order");
        let customer_id = customer.id;
        svc.customers.seed(customer);
        let pm = payment_method(tenant, customer_id, false);
        let pm_id = pm.id;
        svc.payment_methods.seed(pm);

        svc.set_default_payment_method(ctx(tenant), pm_id).await?;

        assert_eq!(
            *log.lock().unwrap_or_else(|p| p.into_inner()),
            vec!["provider", "repository"],
            "Stripe must be updated before the local mirror"
        );
        Ok(())
    }

    #[tokio::test]
    async fn set_default_provider_error_leaves_the_flags_exactly_as_they_were()
    -> Result<(), Box<dyn Error>> {
        // The test that bites a reversed (mirror-first) implementation: with
        // the provider failing, a correct impl never touches the flags,
        // while a reversed one would already have flipped them.
        let log: CallLog = CallLog::default();
        let svc = service_for_ordering(log.clone(), true);
        let tenant = TenantId::new(Uuid::new_v4());
        let customer = linked_customer(tenant, "cus_fail");
        let customer_id = customer.id;
        svc.customers.seed(customer);
        let old_default = payment_method(tenant, customer_id, true);
        let target = payment_method(tenant, customer_id, false);
        let old_id = old_default.id;
        let target_id = target.id;
        svc.payment_methods.seed(old_default);
        svc.payment_methods.seed(target);

        let result = svc.set_default_payment_method(ctx(tenant), target_id).await;

        assert!(matches!(result, Err(DomainError::Provider(_))));
        let rows = svc.payment_methods.list(tenant).await?;
        assert!(
            rows.iter()
                .find(|p| p.id == old_id)
                .ok_or("old row")?
                .is_default,
            "the old default must be untouched when Stripe failed"
        );
        assert!(
            !rows
                .iter()
                .find(|p| p.id == target_id)
                .ok_or("target row")?
                .is_default,
            "the target must not have become default when Stripe failed"
        );
        assert_eq!(
            *log.lock().unwrap_or_else(|p| p.into_inner()),
            vec!["provider"],
            "the repository must not be reached at all after a provider error"
        );
        Ok(())
    }

    #[tokio::test]
    async fn set_default_for_another_tenants_payment_method_is_not_found()
    -> Result<(), Box<dyn Error>> {
        let mine = TenantId::new(Uuid::new_v4());
        let theirs = TenantId::new(Uuid::new_v4());
        let svc = service();
        let their_customer = linked_customer(theirs, "cus_theirs");
        let their_customer_id = their_customer.id;
        svc.customers.seed(their_customer);
        let their_pm = payment_method(theirs, their_customer_id, false);
        let their_pm_id = their_pm.id;
        svc.payment_methods.seed(their_pm);

        let result = svc.set_default_payment_method(ctx(mine), their_pm_id).await;

        assert!(matches!(result, Err(DomainError::NotFound)));
        assert!(
            svc.provider.set_default_calls().is_empty(),
            "the provider must never be called for a cross-tenant id"
        );
        Ok(())
    }

    // --- remove_payment_method (Stripe-first ordering) -----------------

    #[tokio::test]
    async fn remove_detaches_at_stripe_then_soft_deletes() -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        let customer = linked_customer(tenant, "cus_rm");
        let customer_id = customer.id;
        svc.customers.seed(customer);
        let pm = payment_method(tenant, customer_id, true);
        let pm_id = pm.id;
        let stripe_pm_id = pm.stripe_payment_method_id.clone();
        svc.payment_methods.seed(pm);

        svc.remove_payment_method(ctx(tenant), pm_id).await?;

        assert_eq!(svc.provider.detach_calls(), vec![stripe_pm_id]);
        assert_eq!(
            svc.payment_methods.find(tenant, pm_id).await?,
            None,
            "the row must be soft-deleted after a successful detach"
        );
        Ok(())
    }

    #[tokio::test]
    async fn remove_builds_an_audit_entry_naming_the_tenant_target_and_correlation_id()
    -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        let customer = linked_customer(tenant, "cus_rm_audit");
        let customer_id = customer.id;
        svc.customers.seed(customer);
        let pm = payment_method(tenant, customer_id, true);
        let pm_id = pm.id;
        svc.payment_methods.seed(pm);
        let correlation_id = Uuid::new_v4();

        svc.remove_payment_method(RequestContext::new(tenant, correlation_id), pm_id)
            .await?;

        let entries = svc.payment_methods.removed_entries();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.tenant_id().as_uuid(), tenant.as_uuid());
        assert_eq!(entry.actor(), audit::Actor::System);
        assert_eq!(entry.action(), audit::Action::PaymentMethodDetached);
        assert_eq!(
            entry.target(),
            audit::Target::PaymentMethod(audit::TargetId::new(pm_id.as_uuid()))
        );
        assert_eq!(entry.correlation_id().as_uuid(), correlation_id);
        Ok(())
    }

    #[tokio::test]
    async fn remove_calls_the_provider_before_the_repository() -> Result<(), Box<dyn Error>> {
        let log: CallLog = CallLog::default();
        let svc = service_for_ordering(log.clone(), false);
        let tenant = TenantId::new(Uuid::new_v4());
        let customer = linked_customer(tenant, "cus_rm_order");
        let customer_id = customer.id;
        svc.customers.seed(customer);
        let pm = payment_method(tenant, customer_id, false);
        let pm_id = pm.id;
        svc.payment_methods.seed(pm);

        svc.remove_payment_method(ctx(tenant), pm_id).await?;

        assert_eq!(
            *log.lock().unwrap_or_else(|p| p.into_inner()),
            vec!["provider", "repository"],
            "Stripe must detach before the mirror is soft-deleted"
        );
        Ok(())
    }

    #[tokio::test]
    async fn remove_provider_error_leaves_the_row_present() -> Result<(), Box<dyn Error>> {
        // The reversed (mirror-first) implementation would already have
        // soft-deleted the row before the failing detach.
        let log: CallLog = CallLog::default();
        let svc = service_for_ordering(log.clone(), true);
        let tenant = TenantId::new(Uuid::new_v4());
        let customer = linked_customer(tenant, "cus_rm_fail");
        let customer_id = customer.id;
        svc.customers.seed(customer);
        let pm = payment_method(tenant, customer_id, false);
        let pm_id = pm.id;
        svc.payment_methods.seed(pm);

        let result = svc.remove_payment_method(ctx(tenant), pm_id).await;

        assert!(matches!(result, Err(DomainError::Provider(_))));
        assert!(
            svc.payment_methods.find(tenant, pm_id).await?.is_some(),
            "a failed detach must leave the row present, not a mirror that lies"
        );
        assert_eq!(
            *log.lock().unwrap_or_else(|p| p.into_inner()),
            vec!["provider"],
            "the repository must not be reached after a provider error"
        );
        Ok(())
    }

    #[tokio::test]
    async fn removing_an_already_detached_method_is_not_an_error() -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        let customer = linked_customer(tenant, "cus_rm_stale");
        let customer_id = customer.id;
        svc.customers.seed(customer);
        // A newer event already touched the row, so `detach_event` will
        // report it stale rather than soft-delete it.
        let mut pm = payment_method(tenant, customer_id, false);
        pm.last_event_created_at = Some(OffsetDateTime::now_utc() + Duration::hours(1));
        let pm_id = pm.id;
        svc.payment_methods.seed(pm);

        svc.remove_payment_method(ctx(tenant), pm_id).await?;

        assert_eq!(svc.provider.detach_calls().len(), 1);
        // Stale is not an error; the webhook path owns the row now.
        assert!(svc.payment_methods.find(tenant, pm_id).await?.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn remove_for_another_tenants_payment_method_is_not_found() -> Result<(), Box<dyn Error>>
    {
        let mine = TenantId::new(Uuid::new_v4());
        let theirs = TenantId::new(Uuid::new_v4());
        let svc = service();
        let their_customer = linked_customer(theirs, "cus_theirs_rm");
        let their_customer_id = their_customer.id;
        svc.customers.seed(their_customer);
        let their_pm = payment_method(theirs, their_customer_id, false);
        let their_pm_id = their_pm.id;
        svc.payment_methods.seed(their_pm);

        let result = svc.remove_payment_method(ctx(mine), their_pm_id).await;

        assert!(matches!(result, Err(DomainError::NotFound)));
        assert!(
            svc.provider.detach_calls().is_empty(),
            "the provider must never be called for a cross-tenant id"
        );
        Ok(())
    }

    // --- start_checkout_session -------------------------------

    #[tokio::test]
    async fn start_checkout_resolves_the_plan_and_creates_the_session() -> Result<(), Box<dyn Error>>
    {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();
        let plan = plan(tenant, "price_checkout");
        let plan_id = plan.id;
        svc.plans.seed(plan);

        let snapshot = svc
            .start_checkout_session(ctx(tenant), plan_id, "https://ok", "https://cancel")
            .await?;

        assert!(snapshot.url.contains("price_checkout"));
        // ensure_customer minted a customer, and the provider was called for
        // it with the resolved price.
        assert_eq!(svc.provider.create_customer_calls(), 1);
        let calls = svc.provider.checkout_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "price_checkout");
        Ok(())
    }

    #[tokio::test]
    async fn start_checkout_unknown_plan_is_not_found_and_reaches_nothing()
    -> Result<(), Box<dyn Error>> {
        let tenant = TenantId::new(Uuid::new_v4());
        let svc = service();

        let result = svc
            .start_checkout_session(ctx(tenant), PlanId::new(Uuid::new_v4()), "a", "b")
            .await;

        assert!(matches!(result, Err(DomainError::NotFound)));
        // The plan is resolved before ensure_customer, so no Stripe customer
        // was created and no session was attempted.
        assert_eq!(svc.provider.create_customer_calls(), 0);
        assert!(svc.provider.checkout_calls().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn start_checkout_another_tenants_plan_is_not_found() -> Result<(), Box<dyn Error>> {
        let mine = TenantId::new(Uuid::new_v4());
        let theirs = TenantId::new(Uuid::new_v4());
        let svc = service();
        let their_plan = plan(theirs, "price_theirs");
        let their_plan_id = their_plan.id;
        svc.plans.seed(their_plan);

        let result = svc
            .start_checkout_session(ctx(mine), their_plan_id, "a", "b")
            .await;

        assert!(matches!(result, Err(DomainError::NotFound)));
        assert!(svc.provider.checkout_calls().is_empty());
        Ok(())
    }
}
