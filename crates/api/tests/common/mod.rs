// Each file under `tests/` compiles this module into its own binary, and no
// single binary uses every helper here. That is not dead code -- it's shared
// setup for the router-level tests.
#![allow(dead_code)]

use std::sync::Arc;

use api::{ApiError, AppState};
use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use domain::{
    DomainError, Invoice, InvoiceCursor, InvoiceId, InvoicePage, PaymentMethod, Plan, Subscription,
    SubscriptionStatus, TenantId, VerifiedEvent, WebhookReceipt, WebhookVerifier,
};
use service::{EventOutcome, Reads, WebhookHandler};
use uuid::Uuid;

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

/// `AppState` wired for a tenant-scoped route test: real reads, inert webhook
/// deps.
pub fn app_state(reads: StubReads) -> AppState {
    AppState::new(
        Arc::new(UnusedVerifier),
        Arc::new(UnusedHandler),
        Arc::new(reads),
    )
}
