// Each file under `tests/` compiles this module into its own binary, and no
// single binary uses every helper here. That is not dead code -- it's shared
// setup for the extractor-level tests.
#![allow(dead_code)]

use api::{AppState, CheckoutUrls};
use async_trait::async_trait;
use domain::{
    CheckoutSessionSnapshot, DomainError, Invoice, InvoiceCursor, InvoiceId, InvoicePage,
    PaymentMethod, PaymentMethodId, Plan, PlanId, SetupIntentSnapshot, Subscription,
    SubscriptionId, TenantId, VerifiedEvent, WebhookReceipt, WebhookVerifier,
};
use service::{EventOutcome, Reads, WebhookHandler, Writes};

/// The one message every `Unused*` double's error carries -- these fakes
/// exist only so [`unused_state`] can build a complete `AppState`; nothing
/// in the JWT extractor test suite ever reaches them.
const UNUSED: &str = "unused double: the JWT extractor test suite never calls this";

struct UnusedVerifier;

#[async_trait]
impl WebhookVerifier for UnusedVerifier {
    async fn verify_and_record(
        &self,
        _payload: &[u8],
        _signature_header: &str,
    ) -> Result<WebhookReceipt, DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }
}

struct UnusedHandler;

#[async_trait]
impl WebhookHandler for UnusedHandler {
    async fn handle(&self, _event: VerifiedEvent) -> Result<EventOutcome, DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }
}

struct UnusedReads;

#[async_trait]
impl Reads for UnusedReads {
    async fn list_plans(&self, _tenant: TenantId) -> Result<Vec<Plan>, DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }

    async fn get_current_subscription(
        &self,
        _tenant: TenantId,
    ) -> Result<Option<Subscription>, DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }

    async fn list_payment_methods(
        &self,
        _tenant: TenantId,
    ) -> Result<Vec<PaymentMethod>, DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }

    async fn list_invoices(
        &self,
        _tenant: TenantId,
        _after: Option<InvoiceCursor>,
        _limit: u16,
    ) -> Result<InvoicePage, DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }

    async fn get_invoice(&self, _tenant: TenantId, _id: InvoiceId) -> Result<Invoice, DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }
}

struct UnusedWrites;

#[async_trait]
impl Writes for UnusedWrites {
    async fn ensure_customer(&self, _tenant: TenantId) -> Result<String, DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }

    async fn create_setup_intent(
        &self,
        _tenant: TenantId,
    ) -> Result<SetupIntentSnapshot, DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }

    async fn change_plan(
        &self,
        _tenant: TenantId,
        _subscription_id: SubscriptionId,
        _plan_id: PlanId,
    ) -> Result<Subscription, DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }

    async fn cancel_subscription(
        &self,
        _tenant: TenantId,
        _subscription_id: SubscriptionId,
        _at_period_end: bool,
    ) -> Result<Subscription, DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }

    async fn set_default_payment_method(
        &self,
        _tenant: TenantId,
        _payment_method_id: PaymentMethodId,
    ) -> Result<PaymentMethod, DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }

    async fn remove_payment_method(
        &self,
        _tenant: TenantId,
        _payment_method_id: PaymentMethodId,
    ) -> Result<(), DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }

    async fn start_checkout_session(
        &self,
        _tenant: TenantId,
        _plan_id: PlanId,
        _success_url: &str,
        _cancel_url: &str,
    ) -> Result<CheckoutSessionSnapshot, DomainError> {
        Err(DomainError::Provider(UNUSED.to_string()))
    }
}

/// An `AppState` the extractor tests never actually reach into: `DemoTenant`
/// takes `&AppState` only because `FromRequestParts` requires it, and reads
/// nothing from it. Every field is a double that errors loudly if that ever
/// stops being true.
pub fn unused_state() -> AppState {
    AppState::new(
        std::sync::Arc::new(UnusedVerifier),
        std::sync::Arc::new(UnusedHandler),
        std::sync::Arc::new(UnusedReads),
        std::sync::Arc::new(UnusedWrites),
        CheckoutUrls {
            success: "https://example.invalid/success".to_string(),
            cancel: "https://example.invalid/cancel".to_string(),
        },
    )
}
