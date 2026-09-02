//! **Throwaway scaffolding for the Phase 4c Task 18 rehearsal.** Not part of
//! the shipped surface; Phase 4d replaces it with the real thing.
//!
//! `demo`'s `main` serves only `webhook_router`, because mounting
//! `billing_router` needs a concrete tenant extractor and the demo does not
//! get one until Phase 4d's `POST /demo/token`. That leaves a gap the test
//! suite cannot close: the six write routes are covered as *adapter shapes*
//! (against real Stripe, in `crates/stripe-adapter/tests/`) and as *routes*
//! (against stubs, in `crates/api/tests/writes.rs`), but the two halves have
//! never been joined. Nothing exercises
//! HTTP → extractor → `WriteService` → ledger → Stripe → mirror.
//!
//! This example mounts the real `billing_router` over the real
//! `WriteService`, real Postgres repositories and a real
//! `StripeBillingProvider`, with a header-based tenant extractor standing in
//! for the authenticated one. It is the same `HeaderTenant` shape
//! `crates/api/tests/common/mod.rs` uses, for the same reason: all
//! `billing_router` requires of a host is a `T` that rejects with `ApiError`
//! and converts `Into<TenantId>`.
//!
//! **The `x-tenant` header is not authentication.** Anyone who can reach the
//! port can claim any tenant. That is acceptable here and nowhere else: this
//! binds to loopback and exists to be driven by a script on the same machine.
//!
//! ```bash
//! DATABASE_URL=... STRIPE_SECRET_KEY=sk_test_... \
//! STRIPE_WEBHOOK_SIGNING_SECRET=whsec_... \
//! CHECKOUT_SUCCESS_URL=... CHECKOUT_CANCEL_URL=... PORT=8081 \
//! cargo run -p demo --example write_routes
//! ```

use std::env;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;

use api::{ApiError, AppState, CheckoutUrls, billing_router, webhook_router};
use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use domain::{BillingEvent, BillingEventSink, DomainError, SinkError, TenantId, WebhookVerifier};
use persistence::{
    PgCustomerRepository, PgInvoiceRepository, PgOutboundRequestRepository,
    PgPaymentMethodRepository, PgPlanRepository, PgSubscriptionRepository,
    PgWebhookEventRepository, run_migrations,
};
use secrecy::SecretString;
use service::{ReadService, Reads, WebhookHandler, WebhookProcessor, WriteService, Writes};
use sqlx::postgres::PgPoolOptions;
use stripe_adapter::{
    DEFAULT_TOLERANCE, StripeBillingProvider, StripeConfig, StripeWebhookVerifier, WebhookConfig,
};
use tokio::net::TcpListener;
use uuid::Uuid;

/// Reads the tenant from an `x-tenant` header. A missing or unparsable value
/// is a 404-shaped `ApiError`, matching how the real extractor will reject an
/// unauthenticated request -- an unauthenticated caller must not be able to
/// tell "no such tenant" from "not yours".
struct HeaderTenant(TenantId);

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

struct LoggingSink;

#[async_trait]
impl BillingEventSink for LoggingSink {
    async fn handle(&self, event: BillingEvent) -> Result<(), SinkError> {
        tracing::info!(?event, "billing event received");
        Ok(())
    }
}

/// Reads a required variable, returning an error naming it rather than
/// panicking -- the workspace denies `unwrap`/`expect`/`panic`, examples
/// included.
fn required(name: &str) -> Result<String, Box<dyn Error>> {
    match env::var(name) {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => Err(format!("missing required environment variable {name}").into()),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url = required("DATABASE_URL")?;
    let signing_secret = SecretString::from(required("STRIPE_WEBHOOK_SIGNING_SECRET")?);
    let stripe_secret_key = SecretString::from(required("STRIPE_SECRET_KEY")?);
    let checkout_success_url = required("CHECKOUT_SUCCESS_URL")?;
    let checkout_cancel_url = required("CHECKOUT_CANCEL_URL")?;
    let port: u16 = required("PORT")?.parse()?;

    let pool = PgPoolOptions::new().connect(&database_url).await?;
    run_migrations(&pool).await?;

    let verifier: Arc<dyn WebhookVerifier> = Arc::new(StripeWebhookVerifier::new(
        WebhookConfig {
            signing_secret,
            tolerance: DEFAULT_TOLERANCE,
        },
        PgWebhookEventRepository::new(pool.clone()),
    ));
    let handler: Arc<dyn WebhookHandler> = Arc::new(WebhookProcessor::new(
        PgCustomerRepository::new(pool.clone()),
        PgSubscriptionRepository::new(pool.clone()),
        PgInvoiceRepository::new(pool.clone()),
        PgPaymentMethodRepository::new(pool.clone()),
        PgPlanRepository::new(pool.clone()),
        PgWebhookEventRepository::new(pool.clone()),
        LoggingSink,
    ));
    let reads: Arc<dyn Reads> = Arc::new(ReadService::new(
        PgPlanRepository::new(pool.clone()),
        PgSubscriptionRepository::new(pool.clone()),
        PgInvoiceRepository::new(pool.clone()),
        PgPaymentMethodRepository::new(pool.clone()),
    ));
    let provider = StripeBillingProvider::new(
        &StripeConfig {
            secret: stripe_secret_key,
            base_url: None,
        },
        PgOutboundRequestRepository::new(pool.clone()),
    )?;
    let writes: Arc<dyn Writes> = Arc::new(WriteService::new(
        provider,
        PgCustomerRepository::new(pool.clone()),
        PgSubscriptionRepository::new(pool.clone()),
        PgPaymentMethodRepository::new(pool.clone()),
        PgPlanRepository::new(pool.clone()),
    ));

    let state = AppState::new(
        verifier,
        handler,
        reads,
        writes,
        CheckoutUrls {
            success: checkout_success_url,
            cancel: checkout_cancel_url,
        },
    );

    // The whole point of this file: the tenant-scoped router, mounted, over
    // the real services. Merged with the webhook router so one process serves
    // both halves and a script can watch a write route's effect arrive back
    // as a webhook.
    let router = billing_router::<HeaderTenant>(state.clone()).merge(webhook_router(state));

    // Loopback only -- `x-tenant` is not authentication.
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "write_routes example: billing_router + webhook_router mounted");
    axum::serve(listener, router).await?;
    Ok(())
}
