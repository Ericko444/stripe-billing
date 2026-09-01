// Each file under `tests/` compiles this module into its own binary, and no
// single binary uses every helper here. That is not dead code -- it's shared
// setup for the router-level tests.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};

use api::{ApiError, AppState};
use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use domain::{
    DomainError, Invoice, InvoiceCursor, InvoiceId, InvoicePage, PaymentMethod, Plan, PlanId,
    SetupIntentSnapshot, Subscription, SubscriptionId, SubscriptionStatus, TenantId, VerifiedEvent,
    WebhookReceipt, WebhookVerifier,
};
use service::{EventOutcome, Reads, WebhookHandler, Writes};
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

/// Locks a `Mutex`, recovering from poisoning rather than panicking -- the
/// workspace lints deny `unwrap`/`expect`, and a poisoned test double is
/// still readable.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Test tenant extractor: reads the tenant from an `x-tenant` header. Stands
/// in for the host's real (authenticated) extractor -- all `billing_router`
/// needs is a `T` that rejects with [`ApiError`] and converts
/// [`Into<TenantId>`]. A missing or unparsable header is a 404-shaped
/// `ApiError`, matching how the real thing would reject an unauthenticated
/// request.
pub struct HeaderTenant(pub TenantId);

impl<S: Send + Sync> FromRequestParts<S> for HeaderTenant {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let raw = parts
            .headers
            .get("x-tenant")
            .and_then(|value| value.to_str().ok())
            .ok_or(ApiError::from(DomainError::NotFound))?;
        let id = Uuid::parse_str(raw).map_err(|_| ApiError::from(DomainError::NotFound))?;
        Ok(HeaderTenant(TenantId::new(id)))
    }
}

impl From<HeaderTenant> for TenantId {
    fn from(tenant: HeaderTenant) -> Self {
        tenant.0
    }
}

/// A `Reads` that owns flat resource lists and tenant-scopes them on read,
/// the way each real repository's `WHERE tenant_id = $1` does. Grows one
/// list per vertical slice, alongside the `Reads` method it backs.
#[derive(Default)]
pub struct StubReads {
    pub plans: Vec<Plan>,
    pub subscriptions: Vec<Subscription>,
    pub payment_methods: Vec<PaymentMethod>,
    pub invoices: Vec<Invoice>,
}

/// The keyset page `list_page` would return: newest first, seek past
/// `after`, `limit` rows, `next` only if a further row exists. Mirrors
/// `persistence`'s query and `InMemoryInvoices` so the router tests exercise
/// the same contract without a database.
fn keyset_page(mut rows: Vec<Invoice>, after: Option<InvoiceCursor>, limit: u16) -> InvoicePage {
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
    InvoicePage { items: rows, next }
}

#[async_trait]
impl Reads for StubReads {
    async fn list_plans(&self, tenant: TenantId) -> Result<Vec<Plan>, DomainError> {
        Ok(self
            .plans
            .iter()
            .filter(|plan| plan.tenant_id == tenant)
            .cloned()
            .collect())
    }

    async fn get_current_subscription(
        &self,
        tenant: TenantId,
    ) -> Result<Option<Subscription>, DomainError> {
        Ok(self
            .subscriptions
            .iter()
            .filter(|s| s.tenant_id == tenant && s.status != SubscriptionStatus::Canceled)
            .max_by_key(|s| s.created_at)
            .cloned())
    }

    async fn list_payment_methods(
        &self,
        tenant: TenantId,
    ) -> Result<Vec<PaymentMethod>, DomainError> {
        Ok(self
            .payment_methods
            .iter()
            .filter(|pm| pm.tenant_id == tenant && pm.deleted_at.is_none())
            .cloned()
            .collect())
    }

    async fn list_invoices(
        &self,
        tenant: TenantId,
        after: Option<InvoiceCursor>,
        limit: u16,
    ) -> Result<InvoicePage, DomainError> {
        let mine = self
            .invoices
            .iter()
            .filter(|i| i.tenant_id == tenant && i.deleted_at.is_none())
            .cloned()
            .collect();
        Ok(keyset_page(mine, after, limit))
    }

    async fn get_invoice(&self, tenant: TenantId, id: InvoiceId) -> Result<Invoice, DomainError> {
        self.invoices
            .iter()
            .find(|i| i.tenant_id == tenant && i.id == id && i.deleted_at.is_none())
            .cloned()
            .ok_or(DomainError::NotFound)
    }
}

/// The webhook deps `AppState` requires; the tenant-scoped routes never call
/// either, and they fail loudly if something regresses and does.
pub struct UnusedVerifier;

#[async_trait]
impl WebhookVerifier for UnusedVerifier {
    async fn verify_and_record(
        &self,
        _payload: &[u8],
        _signature_header: &str,
    ) -> Result<WebhookReceipt, DomainError> {
        Err(DomainError::WebhookVerification)
    }
}

pub struct UnusedHandler;

#[async_trait]
impl WebhookHandler for UnusedHandler {
    async fn handle(&self, _event: VerifiedEvent) -> Result<EventOutcome, DomainError> {
        Err(DomainError::WebhookVerification)
    }
}

/// A `Writes` the read-route and webhook tests never reach. Every method
/// returns an error, so a regression that routes a read through the write
/// path fails loudly rather than silently passing.
pub struct UnusedWrites;

#[async_trait]
impl Writes for UnusedWrites {
    async fn ensure_customer(&self, _tenant: TenantId) -> Result<String, DomainError> {
        Err(DomainError::Provider(
            "UnusedWrites: the write path is not exercised by this test".to_string(),
        ))
    }

    async fn create_setup_intent(
        &self,
        _tenant: TenantId,
    ) -> Result<SetupIntentSnapshot, DomainError> {
        Err(DomainError::Provider(
            "UnusedWrites: the write path is not exercised by this test".to_string(),
        ))
    }

    async fn change_plan(
        &self,
        _tenant: TenantId,
        _subscription_id: SubscriptionId,
        _plan_id: PlanId,
    ) -> Result<Subscription, DomainError> {
        Err(DomainError::Provider(
            "UnusedWrites: the write path is not exercised by this test".to_string(),
        ))
    }

    async fn cancel_subscription(
        &self,
        _tenant: TenantId,
        _subscription_id: SubscriptionId,
        _at_period_end: bool,
    ) -> Result<Subscription, DomainError> {
        Err(DomainError::Provider(
            "UnusedWrites: the write path is not exercised by this test".to_string(),
        ))
    }
}

/// A `Writes` fake for the write-route tests. Mimics `WriteService`'s
/// tenant->customer bookkeeping in memory: `ensure_customer` links a tenant
/// to a fresh `cus_...` on first call and returns the same id after;
/// `create_setup_intent` resolves the customer first, then hands back a
/// `client_secret` derived from it so a test can tell two tenants' intents
/// apart. `change_plan` and `cancel_subscription` look a seeded subscription
/// up tenant-scoped (via `seed_subscription`), exactly the way
/// `WriteService` does, so a cross-tenant id 404s here too. Every reached
/// call is recorded for assertions -- a call that 404s before mutation is
/// **not** recorded, mirroring "the provider is never called" for a wrong
/// tenant.
#[derive(Default)]
pub struct StubWrites {
    customers: Mutex<HashMap<TenantId, String>>,
    subscriptions: Mutex<Vec<Subscription>>,
    pub ensure_customer_calls: Mutex<Vec<TenantId>>,
    pub setup_intent_calls: Mutex<Vec<(TenantId, String)>>,
    pub change_plan_calls: Mutex<Vec<(TenantId, SubscriptionId, PlanId)>>,
    pub cancel_calls: Mutex<Vec<(TenantId, SubscriptionId, bool)>>,
}

impl StubWrites {
    /// The `(tenant, stripe_customer_id)` pairs passed through
    /// `create_setup_intent`, in call order.
    pub fn setup_intent_calls(&self) -> Vec<(TenantId, String)> {
        lock(&self.setup_intent_calls).clone()
    }

    /// The `(tenant, subscription_id, plan_id)` triples for which
    /// `change_plan` actually reached its "provider" call, in order.
    pub fn change_plan_calls(&self) -> Vec<(TenantId, SubscriptionId, PlanId)> {
        lock(&self.change_plan_calls).clone()
    }

    /// The `(tenant, subscription_id, at_period_end)` triples for which
    /// `cancel_subscription` actually reached its "provider" call, in order.
    pub fn cancel_calls(&self) -> Vec<(TenantId, SubscriptionId, bool)> {
        lock(&self.cancel_calls).clone()
    }

    /// Seeds a subscription row `change_plan`/`cancel_subscription` can find.
    pub fn seed_subscription(&self, subscription: Subscription) {
        lock(&self.subscriptions).push(subscription);
    }
}

#[async_trait]
impl Writes for StubWrites {
    async fn ensure_customer(&self, tenant: TenantId) -> Result<String, DomainError> {
        lock(&self.ensure_customer_calls).push(tenant);
        Ok(lock(&self.customers)
            .entry(tenant)
            .or_insert_with(|| format!("cus_{}", tenant.as_uuid().simple()))
            .clone())
    }

    async fn create_setup_intent(
        &self,
        tenant: TenantId,
    ) -> Result<SetupIntentSnapshot, DomainError> {
        let customer = self.ensure_customer(tenant).await?;
        lock(&self.setup_intent_calls).push((tenant, customer.clone()));
        Ok(SetupIntentSnapshot {
            client_secret: format!("seti_{customer}_secret_test"),
        })
    }

    async fn change_plan(
        &self,
        tenant: TenantId,
        subscription_id: SubscriptionId,
        plan_id: PlanId,
    ) -> Result<Subscription, DomainError> {
        let mut subscriptions = lock(&self.subscriptions);
        let subscription = subscriptions
            .iter_mut()
            .find(|s| s.tenant_id == tenant && s.id == subscription_id)
            .ok_or(DomainError::NotFound)?;
        lock(&self.change_plan_calls).push((tenant, subscription_id, plan_id));
        subscription.plan_id = plan_id;
        subscription.status = SubscriptionStatus::Active;
        Ok(subscription.clone())
    }

    async fn cancel_subscription(
        &self,
        tenant: TenantId,
        subscription_id: SubscriptionId,
        at_period_end: bool,
    ) -> Result<Subscription, DomainError> {
        let mut subscriptions = lock(&self.subscriptions);
        let subscription = subscriptions
            .iter_mut()
            .find(|s| s.tenant_id == tenant && s.id == subscription_id)
            .ok_or(DomainError::NotFound)?;
        if subscription.status == SubscriptionStatus::Canceled {
            return Ok(subscription.clone());
        }
        lock(&self.cancel_calls).push((tenant, subscription_id, at_period_end));
        if at_period_end {
            subscription.cancel_at_period_end = true;
        } else {
            subscription.status = SubscriptionStatus::Canceled;
        }
        Ok(subscription.clone())
    }
}

/// A `tracing` sink that appends every formatted line to a shared buffer, so
/// a test can install it with `tracing::subscriber::set_default` and assert
/// on what was (not) logged during a request.
#[derive(Clone, Default)]
pub struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    /// Everything logged through this sink so far, as a lossy UTF-8 string.
    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&lock(&self.0)).into_owned()
    }
}

impl io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        lock(&self.0).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogs;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// `AppState` wired for a tenant-scoped route test: real reads, inert webhook
/// and write deps.
pub fn app_state(reads: StubReads) -> AppState {
    AppState::new(
        Arc::new(UnusedVerifier),
        Arc::new(UnusedHandler),
        Arc::new(reads),
        Arc::new(UnusedWrites),
    )
}

/// `AppState` for the write-route tests: inert webhook and read deps, a real
/// (caller-supplied) `Writes`.
pub fn app_state_writes(writes: Arc<dyn Writes>) -> AppState {
    AppState::new(
        Arc::new(UnusedVerifier),
        Arc::new(UnusedHandler),
        Arc::new(StubReads::default()),
        writes,
    )
}
