//! Demo composition root: the only crate that names concrete
//! implementations and owns a `main`.
//!
//! Reads and validates its configuration once at startup, builds the
//! Postgres pool, runs all three migrators (billing, audit, identity), wires
//! the billing module's repositories, Stripe adapter and services, wires the
//! identity module's repositories, Argon2id hasher, rate limiter, reset
//! worker and (logging) mailer, and serves one router under `/api/v1`:
//!
//! - `billing_router::<IdentityTenant>` -- billing's tenant-scoped routes,
//!   behind a session from the identity module
//!   ([`IdentityTenant`](demo::identity_tenant::IdentityTenant));
//! - `webhook_router` -- Stripe's webhook, authenticated by its signature;
//! - `identity_router` -- login, the session, tenant selection, password
//!   change, password reset, and the members routes.
//!
//! Over all of it, the identity state (so billing's routes can resolve a
//! session) and the CSRF origin check (so the session cookie cannot be ridden
//! from another site, whichever module's route it is sent to).
//!
//! The workspace denies `unwrap`, `expect` and `panic`; the one documented
//! exception is startup config parsing, and even here
//! it is an explicit early return naming the missing variable, never a bare
//! `unwrap`.

mod seed;

use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;

use api::{AppState, CheckoutUrls, billing_router, webhook_router};
use async_trait::async_trait;
use axum::Router;
use axum::extract::Extension;
use axum::middleware;
use demo::identity_tenant::IdentityTenant;
use demo::log_mailer::LogMailer;
use domain::{BillingEvent, BillingEventSink, SinkError, WebhookVerifier};
use identity_api::{AllowedOrigins, IdentityState, identity_router, origin_check};
use identity_pg::{
    PgMembershipRepository, PgPasswordTokenRepository, PgSessionRepository, PgUserRepository,
};
use identity_service::{
    Argon2Hasher, AuthService, InMemoryRateLimiter, MembersService, PasswordResetService,
    RESET_QUEUE_CAPACITY, SystemClock, reset_queue, run_reset_worker,
};
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
use thiserror::Error;
use tokio::net::TcpListener;

/// Validated startup configuration. Secrets live in [`SecretString`] from the
/// moment they are read, never in a plain `String` that could reach a log.
#[derive(Debug)]
struct Config {
    /// Postgres connection string for every schema: billing, audit, identity.
    database_url: String,
    /// The Stripe webhook endpoint's signing secret (`whsec_...`).
    signing_secret: SecretString,
    /// The Stripe secret API key (`sk_...`) the write path's
    /// `BillingProvider` authenticates outbound calls with.
    stripe_secret_key: SecretString,
    /// Where a Checkout Session returns the customer after success. Host
    /// config, never a request field -- see [`CheckoutUrls`].
    checkout_success_url: String,
    /// Where a Checkout Session returns the customer if they abandon it.
    checkout_cancel_url: String,
    /// TCP port the listener binds.
    port: u16,
    /// Origins a cookie-carrying write may come from (comma-separated in the
    /// environment) -- the frontend's own origin, in the demo the Vite dev
    /// server.
    identity_allowed_origins: Vec<String>,
    /// The one proxy whose `X-Forwarded-For` is believed, if the demo sits
    /// behind one. Unset: the socket peer is the client.
    identity_trusted_proxy: Option<IpAddr>,
    /// The frontend's own origin, which reset and invitation links point
    /// at -- `{this}/reset-password#token=...`.
    identity_public_base_url: String,
}

/// Why startup configuration could not be assembled. Each variant names the
/// offending environment variable so an operator knows what to set.
#[derive(Debug, Error, PartialEq, Eq)]
enum ConfigError {
    /// A required variable is unset or empty.
    #[error("required environment variable {0} is not set")]
    Missing(&'static str),
    /// `PORT` is set but is not a u16.
    #[error("environment variable PORT is not a valid port number: {0:?}")]
    InvalidPort(String),
    /// `IDENTITY_TRUSTED_PROXY` is set but is not an IP address.
    #[error("environment variable IDENTITY_TRUSTED_PROXY is not an IP address: {0:?}")]
    InvalidTrustedProxy(String),
}

impl Config {
    /// Assembles config from a lookup function (`std::env::var` in `main`, a
    /// fixture map in tests). Every value but the trusted proxy is required:
    /// a missing one is a hard startup failure, not a defaulted value.
    fn from_env<F>(get: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let optional = |name: &'static str| get(name).filter(|value| !value.is_empty());
        let required = |name: &'static str| optional(name).ok_or(ConfigError::Missing(name));

        let database_url = required("DATABASE_URL")?;
        let signing_secret = required("STRIPE_WEBHOOK_SIGNING_SECRET")?;
        let stripe_secret_key = required("STRIPE_SECRET_KEY")?;
        let checkout_success_url = required("CHECKOUT_SUCCESS_URL")?;
        let checkout_cancel_url = required("CHECKOUT_CANCEL_URL")?;
        let port_raw = required("PORT")?;
        let port = port_raw
            .parse::<u16>()
            .map_err(|_| ConfigError::InvalidPort(port_raw))?;
        let identity_allowed_origins: Vec<String> = required("IDENTITY_ALLOWED_ORIGINS")?
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_string)
            .collect();
        if identity_allowed_origins.is_empty() {
            return Err(ConfigError::Missing("IDENTITY_ALLOWED_ORIGINS"));
        }
        let identity_public_base_url = required("IDENTITY_PUBLIC_BASE_URL")?;
        let identity_trusted_proxy = optional("IDENTITY_TRUSTED_PROXY")
            .map(|raw| {
                raw.parse::<IpAddr>()
                    .map_err(|_| ConfigError::InvalidTrustedProxy(raw))
            })
            .transpose()?;

        Ok(Config {
            database_url,
            signing_secret: SecretString::from(signing_secret),
            stripe_secret_key: SecretString::from(stripe_secret_key),
            checkout_success_url,
            checkout_cancel_url,
            port,
            identity_allowed_origins,
            identity_trusted_proxy,
            identity_public_base_url,
        })
    }
}

/// The demo's [`BillingEventSink`]: it logs the mirrored event and returns
/// `Ok`. A real host would provision or revoke features here; the demo's job
/// is only to show the event arriving after the mirror write.
struct LoggingSink;

#[async_trait]
impl BillingEventSink for LoggingSink {
    async fn handle(&self, event: BillingEvent) -> Result<(), SinkError> {
        tracing::info!(?event, "billing event received");
        Ok(())
    }
}

/// Builds every concrete implementation, wires them together and serves both
/// modules until the process is killed.
async fn run(config: Config) -> Result<(), Box<dyn Error>> {
    let pool = PgPoolOptions::new().connect(&config.database_url).await?;
    run_migrations(&pool).await?;
    // `audit-pg`'s and `identity-pg`'s own migrators, against the same
    // database -- safe beside the one above because each tracks its state in
    // its own schema's `_sqlx_migrations`, not the default table.
    audit_pg::run_migrations(&pool).await?;
    identity_pg::run_migrations(&pool).await?;

    // Postgres repositories: customer, subscription and invoice lookups plus
    // the webhook ledger for the processor, and a second ledger handle for
    // the verifier's own dedup write.
    let customers = PgCustomerRepository::new(pool.clone());
    let subscriptions = PgSubscriptionRepository::new(pool.clone());
    let invoices = PgInvoiceRepository::new(pool.clone());
    let payment_methods = PgPaymentMethodRepository::new(pool.clone());
    let plans = PgPlanRepository::new(pool.clone());
    let processor_events = PgWebhookEventRepository::new(pool.clone());
    let verifier_events = PgWebhookEventRepository::new(pool.clone());

    let verifier: Arc<dyn WebhookVerifier> = Arc::new(StripeWebhookVerifier::new(
        WebhookConfig {
            signing_secret: config.signing_secret,
            tolerance: DEFAULT_TOLERANCE,
        },
        verifier_events,
    ));
    let handler: Arc<dyn WebhookHandler> = Arc::new(WebhookProcessor::new(
        customers,
        subscriptions,
        invoices,
        payment_methods,
        plans,
        processor_events,
        LoggingSink,
    ));

    // The read service behind the tenant-scoped `GET` routes, from a second
    // set of repository handles (a `PgPool` clone is cheap).
    let reads: Arc<dyn Reads> = Arc::new(ReadService::new(
        PgPlanRepository::new(pool.clone()),
        PgSubscriptionRepository::new(pool.clone()),
        PgInvoiceRepository::new(pool.clone()),
        PgPaymentMethodRepository::new(pool.clone()),
    ));

    // The write service behind the tenant-scoped `POST`/`DELETE` routes: a
    // `StripeBillingProvider` over its own ledger repository handle, plus a
    // third set of mirror repository handles.
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
        audit_pg::PgAuditSink::new(pool.clone()),
    ));

    // The Checkout Session redirect URLs the write path needs. From config,
    // never a request field.
    let checkout_urls = CheckoutUrls {
        success: config.checkout_success_url,
        cancel: config.checkout_cancel_url,
    };
    let state = AppState::new(verifier, handler, reads, writes, checkout_urls);

    // The identity module. The hasher computes its dummy hash here, at
    // startup, so the first login does not pay for it -- and it is one hasher,
    // cloned, so login, password change and reset all draw on the same
    // bounded pool of concurrent hashes and the memory budget holds for all
    // of them together.
    let hasher = Argon2Hasher::new()?;
    let authentication = AuthService::new(
        PgUserRepository::new(pool.clone()),
        PgMembershipRepository::new(pool.clone()),
        PgSessionRepository::new(pool.clone()),
        hasher.clone(),
        SystemClock,
    );
    // Password reset: issuing runs in its own task, fed by a bounded queue --
    // the request handler only enqueues, so its response cannot depend on
    // whether an account exists. Completing runs on the request. The demo
    // mailer logs links instead of sending them -- said loudly, every start.
    let resets = Arc::new(PasswordResetService::new(
        PgUserRepository::new(pool.clone()),
        PgPasswordTokenRepository::new(pool.clone()),
        hasher,
        LogMailer,
        SystemClock,
        config.identity_public_base_url.clone(),
    ));
    let (reset_queue, reset_receiver) = reset_queue(RESET_QUEUE_CAPACITY);
    tokio::spawn(run_reset_worker(reset_receiver, Arc::clone(&resets)));
    tracing::warn!(
        "password reset and invitation links are written to this log by the demo mailer, not sent"
    );

    // Members: adding an address mails an invitation link (or, for an
    // account that has a password, a notice) through the same demo mailer.
    let members = MembersService::new(
        PgMembershipRepository::new(pool.clone()),
        LogMailer,
        SystemClock,
        config.identity_public_base_url,
    );

    let mut identity = IdentityState::new(
        Arc::new(authentication),
        resets,
        Arc::new(members),
        Arc::new(InMemoryRateLimiter::new(SystemClock)),
        reset_queue,
    );
    if let Some(proxy) = config.identity_trusted_proxy {
        identity = identity.with_trusted_proxy(proxy);
    }
    let allowed_origins = AllowedOrigins::new(&config.identity_allowed_origins);

    // One router for both modules. `Extension(identity)` over the merge is
    // what lets `IdentityTenant` on billing's routes resolve a session; the
    // origin check over the merge is what protects billing's writes now that
    // a cookie authenticates them. The webhook route carries no cookie, so
    // the origin check passes it by its own rule, with no carve-out.
    let router = billing_router::<IdentityTenant>(state.clone())
        .merge(webhook_router(state))
        .merge(identity_router(identity.clone()))
        .layer(Extension(identity))
        .layer(middleware::from_fn_with_state(
            allowed_origins,
            origin_check,
        ));

    // The `/api/v1` base path. Every route moves together -- one rule beats a
    // rule plus a carve-out -- so `/webhooks/stripe` moves too. Its
    // signature is computed over the body, never the path, so the forward
    // URL is the only thing that changes: `stripe listen --forward-to
    // localhost:PORT/api/v1/webhooks/stripe`.
    let router = Router::new().nest("/api/v1", router);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "serving billing_router, webhook_router and identity_router");
    // With connect info: the identity module rate-limits by client address,
    // and refuses to serve without one rather than put every caller in one
    // bucket.
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load `.env` if present; real environment always wins.
    dotenvy::dotenv().ok();

    // `demo` has no argument parsing and must not gain a dependency for it:
    // two arms, `seed` and everything else, on the one argument this binary
    // ever takes.
    match std::env::args().nth(1).as_deref() {
        Some("seed") => match seed::run_seed().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("seed failed: {err}");
                ExitCode::FAILURE
            }
        },
        Some(other) => {
            eprintln!("usage: demo [seed]\nunknown argument: {other}");
            ExitCode::FAILURE
        }
        None => {
            let config = match Config::from_env(|key| std::env::var(key).ok()) {
                Ok(config) => config,
                Err(err) => {
                    eprintln!("startup configuration error: {err}");
                    return ExitCode::FAILURE;
                }
            };

            match run(config).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("fatal: {err}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use secrecy::ExposeSecret;

    use super::*;

    /// Builds a lookup closure over a fixed set of key/value pairs.
    fn getter(pairs: Vec<(&'static str, &'static str)>) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        }
    }

    /// Every required variable, in the order `from_env` checks them.
    const FULL_ENV: &[(&str, &str)] = &[
        ("DATABASE_URL", "postgres://localhost/billing"),
        ("STRIPE_WEBHOOK_SIGNING_SECRET", "whsec_abc123"),
        ("STRIPE_SECRET_KEY", "sk_test_abc123"),
        ("CHECKOUT_SUCCESS_URL", "https://app.example/done"),
        ("CHECKOUT_CANCEL_URL", "https://app.example/billing"),
        ("PORT", "8080"),
        (
            "IDENTITY_ALLOWED_ORIGINS",
            "https://app.example, http://localhost:5173",
        ),
        ("IDENTITY_PUBLIC_BASE_URL", "http://localhost:5173"),
    ];

    /// `FULL_ENV` with `name` removed and `extra` added.
    fn env_without(
        name: &str,
        extra: &[(&'static str, &'static str)],
    ) -> Vec<(&'static str, &'static str)> {
        FULL_ENV
            .iter()
            .copied()
            .filter(|(key, _)| *key != name)
            .chain(extra.iter().copied())
            .collect()
    }

    #[test]
    fn all_present_parses() {
        let config = Config::from_env(getter(FULL_ENV.to_vec()));

        assert!(matches!(
            &config,
            Ok(c)
                if c.database_url == "postgres://localhost/billing"
                    && c.port == 8080
                    && c.signing_secret.expose_secret() == "whsec_abc123"
                    && c.stripe_secret_key.expose_secret() == "sk_test_abc123"
                    && c.checkout_success_url == "https://app.example/done"
                    && c.checkout_cancel_url == "https://app.example/billing"
                    && c.identity_allowed_origins
                        == vec!["https://app.example".to_string(), "http://localhost:5173".to_string()]
                    && c.identity_trusted_proxy.is_none()
        ));
    }

    #[test]
    fn debug_redacts_every_secret() {
        let config = Config::from_env(getter(FULL_ENV.to_vec()));
        let rendered = format!("{config:?}");

        assert!(!rendered.contains("whsec_abc123"));
        assert!(!rendered.contains("sk_test_abc123"));
    }

    #[test]
    fn each_required_variable_is_named_when_missing() {
        for name in [
            "DATABASE_URL",
            "STRIPE_WEBHOOK_SIGNING_SECRET",
            "STRIPE_SECRET_KEY",
            "CHECKOUT_SUCCESS_URL",
            "CHECKOUT_CANCEL_URL",
            "PORT",
            "IDENTITY_ALLOWED_ORIGINS",
            "IDENTITY_PUBLIC_BASE_URL",
        ] {
            let result = Config::from_env(getter(env_without(name, &[])));
            assert!(
                matches!(&result, Err(ConfigError::Missing(missing)) if *missing == name),
                "{name}: {result:?}"
            );
        }
    }

    #[test]
    fn empty_value_counts_as_missing() {
        let result = Config::from_env(getter(env_without("DATABASE_URL", &[("DATABASE_URL", "")])));

        assert!(matches!(result, Err(ConfigError::Missing("DATABASE_URL"))));
    }

    #[test]
    fn an_origin_list_of_only_separators_counts_as_missing() {
        let result = Config::from_env(getter(env_without(
            "IDENTITY_ALLOWED_ORIGINS",
            &[("IDENTITY_ALLOWED_ORIGINS", " , ,")],
        )));

        assert!(matches!(
            result,
            Err(ConfigError::Missing("IDENTITY_ALLOWED_ORIGINS"))
        ));
    }

    #[test]
    fn non_numeric_port_is_rejected() {
        let result = Config::from_env(getter(env_without("PORT", &[("PORT", "not-a-number")])));

        assert!(matches!(
            result,
            Err(ConfigError::InvalidPort(raw)) if raw == "not-a-number"
        ));
    }

    #[test]
    fn a_trusted_proxy_is_optional_but_must_be_an_ip_when_set() {
        let valid = Config::from_env(getter(env_without(
            "",
            &[("IDENTITY_TRUSTED_PROXY", "10.0.0.1")],
        )));
        assert!(matches!(
            valid,
            Ok(c) if c.identity_trusted_proxy == Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))
        ));

        let invalid = Config::from_env(getter(env_without(
            "",
            &[("IDENTITY_TRUSTED_PROXY", "proxy.internal")],
        )));
        assert!(matches!(
            invalid,
            Err(ConfigError::InvalidTrustedProxy(raw)) if raw == "proxy.internal"
        ));
    }
}
