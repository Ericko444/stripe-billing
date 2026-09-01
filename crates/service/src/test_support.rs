//! In-memory doubles for the ports `WebhookProcessor` depends on.
//!
//! `#[cfg(test)]`-only: nothing here is reachable outside this crate's own
//! unit tests, so none of it needs to satisfy `missing_docs`. Seeded
//! directly rather than through `create` -- these tests exercise lookups
//! and the ordering guard, not insertion mechanics.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use domain::{
    BillingEvent, BillingEventSink, BillingProvider, CancellationTiming, CheckoutSessionParams,
    CheckoutSessionSnapshot, CreateCustomerParams, Customer, CustomerId, CustomerRepository,
    CustomerSnapshot, DomainError, EventApplication, Invoice, InvoiceCursor, InvoiceId,
    InvoicePage, InvoiceRepository, InvoiceStatus, Money, PaymentMethod, PaymentMethodId,
    PaymentMethodRepository, Plan, PlanId, PlanRepository, SetupIntentSnapshot, SinkError,
    Subscription, SubscriptionId, SubscriptionRepository, SubscriptionSnapshot, SubscriptionStatus,
    TenantId, UpdateCustomerParams, WebhookEvent, WebhookEventId, WebhookEventRepository,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Locks `mutex`, recovering from poisoning rather than panicking -- these
/// doubles never need `.unwrap()`/`.expect()` to reach the guard, and the
/// workspace lints deny both.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A shared, ordered record of *which* double was reached, so the
/// `init-spec.md` §7.4 tests can assert "the provider ran before the
/// repository." Both [`StubBillingProvider`] and [`InMemoryPaymentMethods`]
/// push into the same one -- `"provider"` and `"repository"` respectively.
pub(crate) type CallLog = Arc<Mutex<Vec<&'static str>>>;

#[derive(Default)]
pub(crate) struct InMemoryCustomers {
    rows: Mutex<Vec<Customer>>,
}

impl InMemoryCustomers {
    pub(crate) fn seed(&self, customer: Customer) {
        lock(&self.rows).push(customer);
    }
}

impl CustomerRepository for InMemoryCustomers {
    async fn create(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: Option<String>,
    ) -> Result<Customer, DomainError> {
        let customer = Customer {
            id: CustomerId::new(Uuid::new_v4()),
            tenant_id,
            stripe_customer_id,
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        };
        lock(&self.rows).push(customer.clone());
        Ok(customer)
    }

    async fn find(
        &self,
        tenant_id: TenantId,
        id: CustomerId,
    ) -> Result<Option<Customer>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .find(|c| c.tenant_id == tenant_id && c.id == id && c.deleted_at.is_none())
            .cloned())
    }

    async fn list(&self, tenant_id: TenantId) -> Result<Vec<Customer>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .filter(|c| c.tenant_id == tenant_id && c.deleted_at.is_none())
            .cloned()
            .collect())
    }

    async fn find_by_stripe_customer_id(
        &self,
        stripe_customer_id: &str,
    ) -> Result<Option<Customer>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .find(|c| {
                c.stripe_customer_id.as_deref() == Some(stripe_customer_id)
                    && c.deleted_at.is_none()
            })
            .cloned())
    }
}

#[derive(Default)]
pub(crate) struct InMemorySubscriptions {
    rows: Mutex<Vec<Subscription>>,
}

impl InMemorySubscriptions {
    pub(crate) fn seed(&self, subscription: Subscription) {
        lock(&self.rows).push(subscription);
    }
}

impl SubscriptionRepository for InMemorySubscriptions {
    #[allow(clippy::too_many_arguments)]
    async fn create(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        plan_id: domain::PlanId,
        stripe_subscription_id: String,
        stripe_subscription_item_id: String,
        status: SubscriptionStatus,
        current_period_start: OffsetDateTime,
        current_period_end: OffsetDateTime,
    ) -> Result<Subscription, DomainError> {
        let subscription = Subscription {
            id: SubscriptionId::new(Uuid::new_v4()),
            tenant_id,
            customer_id,
            plan_id,
            stripe_subscription_id,
            stripe_subscription_item_id,
            status,
            current_period_start,
            current_period_end,
            cancel_at_period_end: false,
            last_event_created_at: None,
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        };
        lock(&self.rows).push(subscription.clone());
        Ok(subscription)
    }

    async fn find(
        &self,
        tenant_id: TenantId,
        id: SubscriptionId,
    ) -> Result<Option<Subscription>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .find(|s| s.tenant_id == tenant_id && s.id == id && s.deleted_at.is_none())
            .cloned())
    }

    async fn list(&self, tenant_id: TenantId) -> Result<Vec<Subscription>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .filter(|s| s.tenant_id == tenant_id && s.deleted_at.is_none())
            .cloned()
            .collect())
    }

    async fn find_by_stripe_subscription_id(
        &self,
        tenant_id: TenantId,
        stripe_subscription_id: &str,
    ) -> Result<Option<Subscription>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .find(|s| {
                s.tenant_id == tenant_id
                    && s.stripe_subscription_id == stripe_subscription_id
                    && s.deleted_at.is_none()
            })
            .cloned())
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_event(
        &self,
        tenant_id: TenantId,
        id: SubscriptionId,
        status: SubscriptionStatus,
        current_period_start: OffsetDateTime,
        current_period_end: OffsetDateTime,
        cancel_at_period_end: bool,
        event_created_at: OffsetDateTime,
    ) -> Result<EventApplication, DomainError> {
        // Mirrors the SQL guard exactly (persistence's `apply_event`): admit
        // when there is no prior event, or the new one is not older.
        let mut rows = lock(&self.rows);
        let Some(row) = rows
            .iter_mut()
            .find(|s| s.tenant_id == tenant_id && s.id == id && s.deleted_at.is_none())
        else {
            return Ok(EventApplication::Stale);
        };

        let admitted = match row.last_event_created_at {
            None => true,
            Some(last) => last <= event_created_at,
        };
        if !admitted {
            return Ok(EventApplication::Stale);
        }

        row.status = status;
        row.current_period_start = current_period_start;
        row.current_period_end = current_period_end;
        row.cancel_at_period_end = cancel_at_period_end;
        row.last_event_created_at = Some(event_created_at);
        Ok(EventApplication::Applied)
    }

    async fn set_plan(
        &self,
        tenant_id: TenantId,
        id: SubscriptionId,
        plan_id: PlanId,
    ) -> Result<(), DomainError> {
        // Mirrors the SQL exactly: tenant-scoped, no ordering predicate, and
        // a non-matching row is silently not updated.
        if let Some(row) = lock(&self.rows)
            .iter_mut()
            .find(|s| s.tenant_id == tenant_id && s.id == id && s.deleted_at.is_none())
        {
            row.plan_id = plan_id;
        }
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct InMemoryInvoices {
    rows: Mutex<Vec<Invoice>>,
}

impl InMemoryInvoices {
    pub(crate) fn seed(&self, invoice: Invoice) {
        lock(&self.rows).push(invoice);
    }
}

impl InvoiceRepository for InMemoryInvoices {
    async fn create(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        subscription_id: Option<SubscriptionId>,
        stripe_invoice_id: String,
        amount: Money,
        status: InvoiceStatus,
    ) -> Result<Invoice, DomainError> {
        let invoice = Invoice {
            id: InvoiceId::new(Uuid::new_v4()),
            tenant_id,
            customer_id,
            subscription_id,
            stripe_invoice_id,
            amount,
            status,
            last_event_created_at: None,
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        };
        lock(&self.rows).push(invoice.clone());
        Ok(invoice)
    }

    async fn find(
        &self,
        tenant_id: TenantId,
        id: InvoiceId,
    ) -> Result<Option<Invoice>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .find(|i| i.tenant_id == tenant_id && i.id == id && i.deleted_at.is_none())
            .cloned())
    }

    async fn list(&self, tenant_id: TenantId) -> Result<Vec<Invoice>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .filter(|i| i.tenant_id == tenant_id && i.deleted_at.is_none())
            .cloned()
            .collect())
    }

    async fn list_page(
        &self,
        tenant_id: TenantId,
        after: Option<InvoiceCursor>,
        limit: u16,
    ) -> Result<InvoicePage, DomainError> {
        // Same contract as the Postgres query: newest first, keyset seek on
        // `(created_at, id)`, one row peeked past `limit` to set `next`.
        let mut rows: Vec<Invoice> = lock(&self.rows)
            .iter()
            .filter(|i| i.tenant_id == tenant_id && i.deleted_at.is_none())
            .cloned()
            .collect();
        rows.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.as_uuid().cmp(&a.id.as_uuid()))
        });
        if let Some(cursor) = after {
            rows.retain(|i| {
                (i.created_at, i.id.as_uuid()) < (cursor.created_at(), cursor.id().as_uuid())
            });
        }

        let has_more = rows.len() > usize::from(limit);
        rows.truncate(usize::from(limit));
        let next = has_more
            .then(|| rows.last())
            .flatten()
            .map(|last| InvoiceCursor::new(last.created_at, last.id));

        Ok(InvoicePage { items: rows, next })
    }

    async fn find_by_stripe_invoice_id(
        &self,
        tenant_id: TenantId,
        stripe_invoice_id: &str,
    ) -> Result<Option<Invoice>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .find(|i| {
                i.tenant_id == tenant_id
                    && i.stripe_invoice_id == stripe_invoice_id
                    && i.deleted_at.is_none()
            })
            .cloned())
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_event(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        subscription_id: Option<SubscriptionId>,
        stripe_invoice_id: &str,
        amount: Money,
        status: InvoiceStatus,
        event_created_at: OffsetDateTime,
    ) -> Result<EventApplication, DomainError> {
        // Mirrors persistence's upsert-with-guard: insert if absent, else
        // update only when this event is not older than the last applied.
        let mut rows = lock(&self.rows);
        match rows.iter_mut().find(|i| {
            i.tenant_id == tenant_id
                && i.stripe_invoice_id == stripe_invoice_id
                && i.deleted_at.is_none()
        }) {
            Some(row) => {
                let admitted = match row.last_event_created_at {
                    None => true,
                    Some(last) => last <= event_created_at,
                };
                if !admitted {
                    return Ok(EventApplication::Stale);
                }
                row.customer_id = customer_id;
                row.subscription_id = subscription_id;
                row.amount = amount;
                row.status = status;
                row.last_event_created_at = Some(event_created_at);
                Ok(EventApplication::Applied)
            }
            None => {
                rows.push(Invoice {
                    id: InvoiceId::new(Uuid::new_v4()),
                    tenant_id,
                    customer_id,
                    subscription_id,
                    stripe_invoice_id: stripe_invoice_id.to_string(),
                    amount,
                    status,
                    last_event_created_at: Some(event_created_at),
                    created_at: OffsetDateTime::now_utc(),
                    deleted_at: None,
                });
                Ok(EventApplication::Applied)
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct InMemoryPaymentMethods {
    rows: Mutex<Vec<PaymentMethod>>,
    call_log: Option<CallLog>,
}

impl InMemoryPaymentMethods {
    pub(crate) fn seed(&self, payment_method: PaymentMethod) {
        lock(&self.rows).push(payment_method);
    }

    /// Like `default()`, but `set_default` and `detach_event` push
    /// `"repository"` into `log` when reached -- the §7.4 ordering tests.
    pub(crate) fn with_call_log(log: CallLog) -> Self {
        Self {
            rows: Mutex::default(),
            call_log: Some(log),
        }
    }

    fn record(&self) {
        if let Some(log) = &self.call_log {
            lock(log).push("repository");
        }
    }
}

impl PaymentMethodRepository for InMemoryPaymentMethods {
    async fn create(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        stripe_payment_method_id: String,
        brand: String,
        last4: String,
        is_default: bool,
    ) -> Result<PaymentMethod, DomainError> {
        let pm = PaymentMethod {
            id: PaymentMethodId::new(Uuid::new_v4()),
            tenant_id,
            customer_id,
            stripe_payment_method_id,
            brand,
            last4,
            is_default,
            last_event_created_at: None,
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        };
        lock(&self.rows).push(pm.clone());
        Ok(pm)
    }

    async fn find(
        &self,
        tenant_id: TenantId,
        id: PaymentMethodId,
    ) -> Result<Option<PaymentMethod>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .find(|p| p.tenant_id == tenant_id && p.id == id && p.deleted_at.is_none())
            .cloned())
    }

    async fn list(&self, tenant_id: TenantId) -> Result<Vec<PaymentMethod>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .filter(|p| p.tenant_id == tenant_id && p.deleted_at.is_none())
            .cloned()
            .collect())
    }

    async fn find_by_stripe_payment_method_id(
        &self,
        tenant_id: TenantId,
        stripe_payment_method_id: &str,
    ) -> Result<Option<PaymentMethod>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .find(|p| {
                p.tenant_id == tenant_id
                    && p.stripe_payment_method_id == stripe_payment_method_id
                    && p.deleted_at.is_none()
            })
            .cloned())
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_event(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        stripe_payment_method_id: &str,
        brand: &str,
        last4: &str,
        is_default: bool,
        event_created_at: OffsetDateTime,
    ) -> Result<EventApplication, DomainError> {
        let mut rows = lock(&self.rows);
        match rows.iter_mut().find(|p| {
            p.tenant_id == tenant_id
                && p.stripe_payment_method_id == stripe_payment_method_id
                && p.deleted_at.is_none()
        }) {
            Some(row) => {
                let admitted = match row.last_event_created_at {
                    None => true,
                    Some(last) => last <= event_created_at,
                };
                if !admitted {
                    return Ok(EventApplication::Stale);
                }
                row.customer_id = customer_id;
                row.brand = brand.to_string();
                row.last4 = last4.to_string();
                row.is_default = is_default;
                row.last_event_created_at = Some(event_created_at);
                Ok(EventApplication::Applied)
            }
            None => {
                rows.push(PaymentMethod {
                    id: PaymentMethodId::new(Uuid::new_v4()),
                    tenant_id,
                    customer_id,
                    stripe_payment_method_id: stripe_payment_method_id.to_string(),
                    brand: brand.to_string(),
                    last4: last4.to_string(),
                    is_default,
                    last_event_created_at: Some(event_created_at),
                    created_at: OffsetDateTime::now_utc(),
                    deleted_at: None,
                });
                Ok(EventApplication::Applied)
            }
        }
    }

    async fn set_default(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        id: PaymentMethodId,
    ) -> Result<(), DomainError> {
        self.record();
        // Mirror the SQL: is_default = (id == target) across the customer's
        // live rows, in one pass -- no "two defaults or none" window.
        for row in lock(&self.rows).iter_mut().filter(|p| {
            p.tenant_id == tenant_id && p.customer_id == customer_id && p.deleted_at.is_none()
        }) {
            row.is_default = row.id == id;
        }
        Ok(())
    }

    async fn detach_event(
        &self,
        tenant_id: TenantId,
        stripe_payment_method_id: &str,
        event_created_at: OffsetDateTime,
    ) -> Result<EventApplication, DomainError> {
        self.record();
        let mut rows = lock(&self.rows);
        let Some(row) = rows.iter_mut().find(|p| {
            p.tenant_id == tenant_id
                && p.stripe_payment_method_id == stripe_payment_method_id
                && p.deleted_at.is_none()
        }) else {
            return Ok(EventApplication::Stale);
        };
        let admitted = match row.last_event_created_at {
            None => true,
            Some(last) => last <= event_created_at,
        };
        if !admitted {
            return Ok(EventApplication::Stale);
        }
        row.deleted_at = Some(OffsetDateTime::now_utc());
        row.last_event_created_at = Some(event_created_at);
        Ok(EventApplication::Applied)
    }
}

#[derive(Default)]
pub(crate) struct InMemoryPlans {
    rows: Mutex<Vec<Plan>>,
}

impl InMemoryPlans {
    pub(crate) fn seed(&self, plan: Plan) {
        lock(&self.rows).push(plan);
    }
}

impl PlanRepository for InMemoryPlans {
    async fn create(
        &self,
        tenant_id: TenantId,
        stripe_price_id: String,
        stripe_product_id: String,
        name: String,
        amount: Money,
    ) -> Result<Plan, DomainError> {
        let plan = Plan {
            id: PlanId::new(Uuid::new_v4()),
            tenant_id,
            stripe_price_id,
            stripe_product_id,
            name,
            amount,
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        };
        lock(&self.rows).push(plan.clone());
        Ok(plan)
    }

    async fn find(&self, tenant_id: TenantId, id: PlanId) -> Result<Option<Plan>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .find(|p| p.tenant_id == tenant_id && p.id == id && p.deleted_at.is_none())
            .cloned())
    }

    async fn list(&self, tenant_id: TenantId) -> Result<Vec<Plan>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .filter(|p| p.tenant_id == tenant_id && p.deleted_at.is_none())
            .cloned()
            .collect())
    }

    async fn find_by_stripe_price_id(
        &self,
        tenant_id: TenantId,
        stripe_price_id: &str,
    ) -> Result<Option<Plan>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .find(|p| {
                p.tenant_id == tenant_id
                    && p.stripe_price_id == stripe_price_id
                    && p.deleted_at.is_none()
            })
            .cloned())
    }
}

#[derive(Default)]
pub(crate) struct InMemoryWebhookEvents {
    rows: Mutex<Vec<WebhookEvent>>,
}

impl InMemoryWebhookEvents {
    pub(crate) fn seed(&self, event: WebhookEvent) {
        lock(&self.rows).push(event);
    }

    /// Reads back `processed_at` for `id`, so a test can assert whether
    /// `mark_processed` ran without going through `find_by_stripe_event_id`.
    pub(crate) fn processed_at(&self, id: WebhookEventId) -> Option<Option<OffsetDateTime>> {
        lock(&self.rows)
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.processed_at)
    }
}

impl WebhookEventRepository for InMemoryWebhookEvents {
    async fn create(
        &self,
        tenant_id: Option<TenantId>,
        stripe_event_id: String,
        event_type: String,
        payload: serde_json::Value,
    ) -> Result<WebhookEvent, DomainError> {
        let event = WebhookEvent {
            id: WebhookEventId::new(Uuid::new_v4()),
            tenant_id,
            stripe_event_id,
            event_type,
            payload,
            created_at: OffsetDateTime::now_utc(),
            processed_at: None,
        };
        lock(&self.rows).push(event.clone());
        Ok(event)
    }

    async fn find_by_stripe_event_id(
        &self,
        stripe_event_id: &str,
    ) -> Result<Option<WebhookEvent>, DomainError> {
        Ok(lock(&self.rows)
            .iter()
            .find(|e| e.stripe_event_id == stripe_event_id)
            .cloned())
    }

    async fn mark_processed(&self, id: WebhookEventId) -> Result<(), DomainError> {
        if let Some(row) = lock(&self.rows).iter_mut().find(|e| e.id == id) {
            row.processed_at = Some(OffsetDateTime::now_utc());
        }
        Ok(())
    }
}

/// Records every [`BillingEvent`] it receives, and can be configured to fail
/// every call (the "a failing sink leaves the mirror written and
/// `processed_at` NULL" scenario).
#[derive(Default)]
pub(crate) struct InMemorySink {
    events: Mutex<Vec<BillingEvent>>,
    fail: bool,
}

impl InMemorySink {
    pub(crate) fn failing() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            fail: true,
        }
    }

    pub(crate) fn received(&self) -> Vec<BillingEvent> {
        lock(&self.events).clone()
    }
}

#[async_trait]
impl BillingEventSink for InMemorySink {
    async fn handle(&self, event: BillingEvent) -> Result<(), SinkError> {
        if self.fail {
            return Err(SinkError("simulated sink failure".to_string()));
        }
        lock(&self.events).push(event);
        Ok(())
    }
}

/// A [`BillingProvider`] double for the write-path use-case tests.
///
/// Counts `create_customer` calls so a test can assert the provider was
/// **not** reached (the "already linked" path of `ensure_customer`), and
/// records the inputs passed to `create_setup_intent`, `change_plan` and
/// `cancel_subscription` so a test can assert what the use case actually
/// sent. Returns deterministic snapshots -- `create_subscription` and
/// `update_customer` have no caller among the use cases wired so far, and
/// return a `Provider` error rather than panic, which the workspace lints
/// deny.
#[derive(Default)]
pub(crate) struct StubBillingProvider {
    create_customer_calls: AtomicUsize,
    setup_intent_customers: Mutex<Vec<String>>,
    change_plan_calls: Mutex<Vec<(String, String, String)>>,
    cancel_calls: Mutex<Vec<(String, CancellationTiming)>>,
    set_default_calls: Mutex<Vec<(String, String)>>,
    detach_calls: Mutex<Vec<String>>,
    call_log: Option<CallLog>,
    fail_payment_method_ops: bool,
}

impl StubBillingProvider {
    /// A double for the §7.4 ordering tests: pushes `"provider"` into `log`
    /// when a payment-method call is reached, and -- when `fail` -- returns a
    /// `Provider` error from it. Pairing this (`fail = true`) with a check
    /// that the mirror did not move is what catches a reversed
    /// (mirror-first) implementation.
    pub(crate) fn for_ordering_test(log: CallLog, fail: bool) -> Self {
        Self {
            call_log: Some(log),
            fail_payment_method_ops: fail,
            ..Self::default()
        }
    }

    /// The `(stripe_customer_id, stripe_payment_method_id)` pairs passed to
    /// `set_default_payment_method`, in order. Empty if it was never reached.
    pub(crate) fn set_default_calls(&self) -> Vec<(String, String)> {
        lock(&self.set_default_calls).clone()
    }

    /// The `stripe_payment_method_id`s passed to `detach_payment_method`, in
    /// order. Empty if it was never reached.
    pub(crate) fn detach_calls(&self) -> Vec<String> {
        lock(&self.detach_calls).clone()
    }

    /// How many times `create_customer` has been called on this double.
    pub(crate) fn create_customer_calls(&self) -> usize {
        self.create_customer_calls.load(Ordering::SeqCst)
    }

    /// The `stripe_customer_id`s passed to `create_setup_intent`, in order.
    pub(crate) fn setup_intent_customers(&self) -> Vec<String> {
        lock(&self.setup_intent_customers).clone()
    }

    /// The `(stripe_subscription_id, stripe_subscription_item_id,
    /// new_stripe_price_id)` triples passed to `change_plan`, in order.
    pub(crate) fn change_plan_calls(&self) -> Vec<(String, String, String)> {
        lock(&self.change_plan_calls).clone()
    }

    /// The `(stripe_subscription_id, timing)` pairs passed to
    /// `cancel_subscription`, in order.
    pub(crate) fn cancel_calls(&self) -> Vec<(String, CancellationTiming)> {
        lock(&self.cancel_calls).clone()
    }
}

#[async_trait]
impl BillingProvider for StubBillingProvider {
    async fn create_customer(
        &self,
        _tenant_id: TenantId,
        _params: CreateCustomerParams,
    ) -> Result<CustomerSnapshot, DomainError> {
        let n = self.create_customer_calls.fetch_add(1, Ordering::SeqCst);
        Ok(CustomerSnapshot {
            stripe_customer_id: format!("cus_stub_{n}"),
        })
    }

    async fn update_customer(
        &self,
        _tenant_id: TenantId,
        _stripe_customer_id: &str,
        _params: UpdateCustomerParams,
    ) -> Result<CustomerSnapshot, DomainError> {
        Err(DomainError::Provider(
            "update_customer not stubbed".to_string(),
        ))
    }

    async fn create_subscription(
        &self,
        _tenant_id: TenantId,
        _stripe_customer_id: &str,
        _stripe_price_id: &str,
    ) -> Result<SubscriptionSnapshot, DomainError> {
        Err(DomainError::Provider(
            "create_subscription not stubbed".to_string(),
        ))
    }

    async fn change_plan(
        &self,
        _tenant_id: TenantId,
        stripe_subscription_id: &str,
        stripe_subscription_item_id: &str,
        new_stripe_price_id: &str,
    ) -> Result<SubscriptionSnapshot, DomainError> {
        lock(&self.change_plan_calls).push((
            stripe_subscription_id.to_string(),
            stripe_subscription_item_id.to_string(),
            new_stripe_price_id.to_string(),
        ));
        Ok(SubscriptionSnapshot {
            stripe_subscription_id: stripe_subscription_id.to_string(),
            stripe_subscription_item_id: stripe_subscription_item_id.to_string(),
            status: SubscriptionStatus::Active,
            current_period_start: OffsetDateTime::now_utc(),
            current_period_end: OffsetDateTime::now_utc() + Duration::days(30),
            cancel_at_period_end: false,
        })
    }

    async fn cancel_subscription(
        &self,
        _tenant_id: TenantId,
        stripe_subscription_id: &str,
        timing: CancellationTiming,
    ) -> Result<SubscriptionSnapshot, DomainError> {
        lock(&self.cancel_calls).push((stripe_subscription_id.to_string(), timing));
        let (status, cancel_at_period_end) = match timing {
            CancellationTiming::AtPeriodEnd => (SubscriptionStatus::Active, true),
            CancellationTiming::Immediate => (SubscriptionStatus::Canceled, false),
        };
        Ok(SubscriptionSnapshot {
            stripe_subscription_id: stripe_subscription_id.to_string(),
            stripe_subscription_item_id: "si_stub".to_string(),
            status,
            current_period_start: OffsetDateTime::now_utc(),
            current_period_end: OffsetDateTime::now_utc() + Duration::days(30),
            cancel_at_period_end,
        })
    }

    async fn create_setup_intent(
        &self,
        _tenant_id: TenantId,
        stripe_customer_id: &str,
    ) -> Result<SetupIntentSnapshot, DomainError> {
        lock(&self.setup_intent_customers).push(stripe_customer_id.to_string());
        Ok(SetupIntentSnapshot {
            client_secret: format!("seti_for_{stripe_customer_id}_secret_stub"),
        })
    }

    async fn set_default_payment_method(
        &self,
        _tenant_id: TenantId,
        stripe_customer_id: &str,
        stripe_payment_method_id: &str,
    ) -> Result<(), DomainError> {
        if let Some(log) = &self.call_log {
            lock(log).push("provider");
        }
        lock(&self.set_default_calls).push((
            stripe_customer_id.to_string(),
            stripe_payment_method_id.to_string(),
        ));
        if self.fail_payment_method_ops {
            return Err(DomainError::Provider(
                "simulated Stripe failure".to_string(),
            ));
        }
        Ok(())
    }

    async fn detach_payment_method(
        &self,
        _tenant_id: TenantId,
        stripe_payment_method_id: &str,
    ) -> Result<(), DomainError> {
        if let Some(log) = &self.call_log {
            lock(log).push("provider");
        }
        lock(&self.detach_calls).push(stripe_payment_method_id.to_string());
        if self.fail_payment_method_ops {
            return Err(DomainError::Provider(
                "simulated Stripe failure".to_string(),
            ));
        }
        Ok(())
    }

    // Task 16 replaces this with a call-recording version; for now it only
    // needs to satisfy the trait and hand back a deterministic snapshot.
    async fn create_checkout_session(
        &self,
        _tenant_id: TenantId,
        params: CheckoutSessionParams,
    ) -> Result<CheckoutSessionSnapshot, DomainError> {
        Ok(CheckoutSessionSnapshot {
            url: format!(
                "https://checkout.stripe.com/c/pay/cs_stub_{}",
                params.stripe_price_id
            ),
            stripe_session_id: "cs_stub".to_string(),
        })
    }
}
