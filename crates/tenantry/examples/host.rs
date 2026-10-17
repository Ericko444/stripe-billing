//! The whole composition, written as a consumer of `tenantry` writes it:
//! both modules, their Postgres adapters and Stripe, served as one router
//! under `/api/v1`.
//!
//! ```bash
//! cargo run -p tenantry --example host --features full
//! ```
//!
//! It reads its configuration from the environment, runs all three
//! migrators against one database, wires billing (repositories, the Stripe
//! adapter, the services) and identity (repositories, the Argon2id hasher,
//! the rate limiter, the reset worker, a mailer), and merges
//! `billing_router::<IdentityTenant>`, `webhook_router` and `identity_router`
//! under the identity state and the CSRF origin check.
//!
//! Two things here are yours to replace before production. The mailer below
//! only logs: no `Mailer` that sends mail ships with `tenantry`, and without
//! one password reset and invitations never arrive. And the rate limiter
//! counts per process, so more than one process needs a shared one.
//!
//! `axum` and `sqlx` are not re-exported; a consumer names them in their own
//! manifest (`axum` 0.8, `sqlx` 0.9).

use std::error::Error;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::extract::Extension;
use axum::middleware;
use secrecy::{ExposeSecret, SecretString};
use sqlx::postgres::PgPoolOptions;
use tenantry::IdentityTenant;
use tenantry::audit_postgres::PgAuditSink;
use tenantry::billing::api::{AppState, CheckoutUrls, billing_router, webhook_router};
use tenantry::billing::domain::{BillingEvent, BillingEventSink, SinkError, WebhookVerifier};
use tenantry::billing::postgres::{
    PgCustomerRepository, PgInvoiceRepository, PgOutboundRequestRepository,
    PgPaymentMethodRepository, PgPlanRepository, PgSubscriptionRepository,
    PgWebhookEventRepository,
};
use tenantry::billing::service::{
    ReadService, Reads, WebhookHandler, WebhookProcessor, WriteService, Writes,
};
use tenantry::billing::stripe::{
    DEFAULT_TOLERANCE, StripeBillingProvider, StripeConfig, StripeWebhookVerifier, WebhookConfig,
};
use tenantry::identity::api::{AllowedOrigins, IdentityState, identity_router, origin_check};
use tenantry::identity::domain::{MailError, Mailer, OutgoingMail};
use tenantry::identity::postgres::{
    PgMembershipRepository, PgPasswordTokenRepository, PgSessionRepository, PgUserRepository,
};
use tenantry::identity::service::{
    Argon2Hasher, AuthService, DeactivationService, InMemoryRateLimiter, MembersService,
    PasswordResetService, RESET_QUEUE_CAPACITY, SystemClock, reset_queue, run_reset_worker,
};
use tokio::net::TcpListener;

/// Startup configuration. The two Stripe secrets go into [`SecretString`] as
/// they are read, so neither can reach a log through `Debug`.
struct Config {
    database_url: String,
    stripe_secret_key: SecretString,
    stripe_webhook_signing_secret: SecretString,
    checkout_success_url: String,
    checkout_cancel_url: String,
    /// Origins a cookie-carrying write may come from: your frontend's.
    allowed_origins: Vec<String>,
    /// Where reset and invitation links point: `{this}/reset-password#token=...`.
    public_base_url: String,
    port: u16,
}

impl Config {
    fn from_env() -> Result<Self, Box<dyn Error>> {
        fn required(name: &str) -> Result<String, Box<dyn Error>> {
            match std::env::var(name) {
                Ok(value) if !value.is_empty() => Ok(value),
                _ => Err(format!("required environment variable {name} is not set").into()),
            }
        }

        Ok(Config {
            database_url: required("DATABASE_URL")?,
            stripe_secret_key: SecretString::from(required("STRIPE_SECRET_KEY")?),
            stripe_webhook_signing_secret: SecretString::from(required(
                "STRIPE_WEBHOOK_SIGNING_SECRET",
            )?),
            checkout_success_url: required("CHECKOUT_SUCCESS_URL")?,
            checkout_cancel_url: required("CHECKOUT_CANCEL_URL")?,
            allowed_origins: required("ALLOWED_ORIGINS")?
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(str::to_string)
                .collect(),
            public_base_url: required("PUBLIC_BASE_URL")?,
            port: required("PORT")?.parse()?,
        })
    }
}

/// Where billing tells you a subscription changed, after its mirror is
/// written. This is where you would provision or revoke features.
struct LoggingSink;

#[async_trait]
impl BillingEventSink for LoggingSink {
    async fn handle(&self, event: BillingEvent) -> Result<(), SinkError> {
        tracing::info!(?event, "billing event received");
        Ok(())
    }
}

/// **Not for production.** Writes each mail, link included, to the log
/// instead of sending it. A link in a log is a password reset anyone with log
/// access can follow. A real `Mailer` hands `mail` to your mail provider and
/// logs, at most, that a mail went out.
struct LoggingMailer;

impl Mailer for LoggingMailer {
    async fn send(&self, mail: &OutgoingMail) -> Result<(), MailError> {
        tracing::warn!(
            to = %mail.to.as_str(),
            purpose = ?mail.purpose,
            link = %mail.link.expose_secret(),
            "NOT SENT: example mailer, logged instead"
        );
        Ok(())
    }
}

async fn run(config: Config) -> Result<(), Box<dyn Error>> {
    let pool = PgPoolOptions::new().connect(&config.database_url).await?;
    // Three migrators, one database: each tracks its state in its own
    // schema's `_sqlx_migrations`, so they do not collide.
    tenantry::billing::postgres::run_migrations(&pool).await?;
    tenantry::audit_postgres::run_migrations(&pool).await?;
    tenantry::identity::postgres::run_migrations(&pool).await?;

    // Billing. A `PgPool` clone is a handle, so each service gets its own
    // repositories.
    let verifier: Arc<dyn WebhookVerifier> = Arc::new(StripeWebhookVerifier::new(
        WebhookConfig {
            signing_secret: config.stripe_webhook_signing_secret,
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
            secret: config.stripe_secret_key,
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
        PgAuditSink::new(pool.clone()),
    ));
    let checkout_urls = CheckoutUrls {
        success: config.checkout_success_url,
        cancel: config.checkout_cancel_url,
    };
    let billing = AppState::new(verifier, handler, reads, writes, checkout_urls);

    // Identity. One hasher, cloned, so every path that hashes shares one
    // bounded pool and one memory budget.
    let hasher = Argon2Hasher::new()?;
    let authentication = AuthService::new(
        PgUserRepository::new(pool.clone()),
        PgMembershipRepository::new(pool.clone()),
        PgSessionRepository::new(pool.clone()),
        hasher.clone(),
        SystemClock,
    );
    let resets = Arc::new(PasswordResetService::new(
        PgUserRepository::new(pool.clone()),
        PgPasswordTokenRepository::new(pool.clone()),
        hasher,
        LoggingMailer,
        SystemClock,
        config.public_base_url.clone(),
    ));
    // Issuing a reset runs off the request, so the response cannot reveal
    // whether the account exists. The worker drains the queue.
    let (reset_sender, reset_receiver) = reset_queue(RESET_QUEUE_CAPACITY);
    tokio::spawn(run_reset_worker(reset_receiver, Arc::clone(&resets)));
    let members = MembersService::new(
        PgMembershipRepository::new(pool.clone()),
        LoggingMailer,
        SystemClock,
        config.public_base_url,
    );
    let deactivations = DeactivationService::new(
        PgMembershipRepository::new(pool.clone()),
        PgUserRepository::new(pool.clone()),
        SystemClock,
    );
    let identity = IdentityState::new(
        Arc::new(authentication),
        resets,
        Arc::new(members),
        Arc::new(deactivations),
        // Counts per process. Run more than one and you need a shared one.
        Arc::new(InMemoryRateLimiter::new(SystemClock)),
        reset_sender,
    );

    // One router. `Extension(identity)` over the merge is what lets
    // `IdentityTenant` resolve a session on billing's routes; the origin
    // check over the merge protects billing's writes, which a cookie now
    // authenticates. The webhook carries no cookie, so the check passes it.
    let api = billing_router::<IdentityTenant>(billing.clone())
        .merge(webhook_router(billing))
        .merge(identity_router(identity.clone()))
        .layer(Extension(identity))
        .layer(middleware::from_fn_with_state(
            AllowedOrigins::new(&config.allowed_origins),
            origin_check,
        ));
    let app = Router::new().nest("/api/v1", api);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "serving /api/v1");
    // Connect info is required: identity rate-limits by client address and
    // refuses to serve without one.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt().init();

    let result = match Config::from_env() {
        Ok(config) => run(config).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("fatal: {err}");
            ExitCode::FAILURE
        }
    }
}
