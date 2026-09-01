//! Demo composition root: the only crate that names concrete
//! implementations and owns a `main`.
//!
//! Reads and validates its configuration once at startup, builds the
//! Postgres pool, runs migrations, wires the four repositories, the Stripe
//! webhook verifier and a logging [`BillingEventSink`] into a
//! [`WebhookProcessor`], and serves `api`'s `billing_router`.
//!
//! The workspace denies `unwrap`, `expect` and `panic`; the one documented
//! exception (`init-spec.md` §5.5) is startup config parsing, and even here
//! it is an explicit early return naming the missing variable, never a bare
//! `unwrap`.

use std::error::Error;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use api::{AppState, billing_router};
use async_trait::async_trait;
use domain::{BillingEvent, BillingEventSink, SinkError, WebhookVerifier};
use persistence::{
    PgCustomerRepository, PgInvoiceRepository, PgPaymentMethodRepository, PgSubscriptionRepository,
    PgWebhookEventRepository, run_migrations,
};
use secrecy::SecretString;
use service::{WebhookHandler, WebhookProcessor};
use sqlx::postgres::PgPoolOptions;
use stripe_adapter::{DEFAULT_TOLERANCE, StripeWebhookVerifier, WebhookConfig};
use thiserror::Error;
use tokio::net::TcpListener;

/// Validated startup configuration. Secrets live in [`SecretString`] from the
/// moment they are read, never in a plain `String` that could reach a log.
#[derive(Debug)]
struct Config {
    /// Postgres connection string for the mirror and ledger tables.
    database_url: String,
    /// The Stripe webhook endpoint's signing secret (`whsec_...`).
    signing_secret: SecretString,
    /// TCP port the webhook listener binds.
    port: u16,
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
}

impl Config {
    /// Assembles config from a lookup function (`std::env::var` in `main`, a
    /// fixture map in tests). All three values are required: a missing one is
    /// a hard startup failure, not a defaulted value.
    fn from_env<F>(get: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let required = |name: &'static str| {
            get(name)
                .filter(|value| !value.is_empty())
                .ok_or(ConfigError::Missing(name))
        };

        let database_url = required("DATABASE_URL")?;
        let signing_secret = required("STRIPE_WEBHOOK_SIGNING_SECRET")?;
        let port_raw = required("PORT")?;
        let port = port_raw
            .parse::<u16>()
            .map_err(|_| ConfigError::InvalidPort(port_raw))?;

        Ok(Config {
            database_url,
            signing_secret: SecretString::from(signing_secret),
            port,
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

/// Builds every concrete implementation, wires them together and serves the
/// billing router until the process is killed.
async fn run(config: Config) -> Result<(), Box<dyn Error>> {
    let pool = PgPoolOptions::new().connect(&config.database_url).await?;
    run_migrations(&pool).await?;

    // Postgres repositories: customer, subscription and invoice lookups plus
    // the webhook ledger for the processor, and a second ledger handle for
    // the verifier's own dedup write.
    let customers = PgCustomerRepository::new(pool.clone());
    let subscriptions = PgSubscriptionRepository::new(pool.clone());
    let invoices = PgInvoiceRepository::new(pool.clone());
    let payment_methods = PgPaymentMethodRepository::new(pool.clone());
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
        processor_events,
        LoggingSink,
    ));

    let router = billing_router(AppState::new(verifier, handler));

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "serving POST /webhooks/stripe");
    axum::serve(listener, router).await?;
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

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    /// Builds a lookup closure over a fixed set of key/value pairs.
    fn getter(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn all_three_present_parses() {
        let config = Config::from_env(getter(&[
            ("DATABASE_URL", "postgres://localhost/billing"),
            ("STRIPE_WEBHOOK_SIGNING_SECRET", "whsec_abc123"),
            ("PORT", "8080"),
        ]));

        assert!(matches!(
            &config,
            Ok(c)
                if c.database_url == "postgres://localhost/billing"
                    && c.port == 8080
                    && c.signing_secret.expose_secret() == "whsec_abc123"
        ));
    }

    #[test]
    fn missing_database_url_is_named() {
        let result = Config::from_env(getter(&[
            ("STRIPE_WEBHOOK_SIGNING_SECRET", "whsec_abc123"),
            ("PORT", "8080"),
        ]));

        assert!(matches!(result, Err(ConfigError::Missing("DATABASE_URL"))));
    }

    #[test]
    fn missing_signing_secret_is_named() {
        let result = Config::from_env(getter(&[
            ("DATABASE_URL", "postgres://localhost/billing"),
            ("PORT", "8080"),
        ]));

        assert!(matches!(
            result,
            Err(ConfigError::Missing("STRIPE_WEBHOOK_SIGNING_SECRET"))
        ));
    }

    #[test]
    fn missing_port_is_named() {
        let result = Config::from_env(getter(&[
            ("DATABASE_URL", "postgres://localhost/billing"),
            ("STRIPE_WEBHOOK_SIGNING_SECRET", "whsec_abc123"),
        ]));

        assert!(matches!(result, Err(ConfigError::Missing("PORT"))));
    }

    #[test]
    fn empty_value_counts_as_missing() {
        let result = Config::from_env(getter(&[
            ("DATABASE_URL", ""),
            ("STRIPE_WEBHOOK_SIGNING_SECRET", "whsec_abc123"),
            ("PORT", "8080"),
        ]));

        assert!(matches!(result, Err(ConfigError::Missing("DATABASE_URL"))));
    }

    #[test]
    fn non_numeric_port_is_rejected() {
        let result = Config::from_env(getter(&[
            ("DATABASE_URL", "postgres://localhost/billing"),
            ("STRIPE_WEBHOOK_SIGNING_SECRET", "whsec_abc123"),
            ("PORT", "not-a-number"),
        ]));

        assert!(matches!(
            result,
            Err(ConfigError::InvalidPort(raw)) if raw == "not-a-number"
        ));
    }
}
