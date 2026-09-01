//! In-memory doubles for the ports `WebhookProcessor` depends on.
//!
//! `#[cfg(test)]`-only: nothing here is reachable outside this crate's own
//! unit tests, so none of it needs to satisfy `missing_docs`. Seeded
//! directly rather than through `create` -- these tests exercise lookups
//! and the ordering guard, not insertion mechanics.

use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;
use domain::{
    BillingEvent, BillingEventSink, Customer, CustomerId, CustomerRepository, DomainError,
    EventApplication, Invoice, InvoiceId, InvoiceRepository, InvoiceStatus, Money, SinkError,
    Subscription, SubscriptionId, SubscriptionRepository, SubscriptionStatus, TenantId,
    WebhookEvent, WebhookEventId, WebhookEventRepository,
};
use time::OffsetDateTime;
use uuid::Uuid;

/// Locks `mutex`, recovering from poisoning rather than panicking -- these
/// doubles never need `.unwrap()`/`.expect()` to reach the guard, and the
/// workspace lints deny both.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
