// Each file under `tests/` compiles this module into its own binary, and no
// single binary uses every helper here (only the cases-D/E tests call
// `backdate`, for instance). That is not dead code -- it's shared setup.
#![allow(dead_code)]

use std::error::Error;

use domain::OutboundRequestId;
use persistence::PgOutboundRequestRepository;
use secrecy::SecretString;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use stripe_adapter::StripeConfig;
use testcontainers::core::wait::LogWaitStrategy;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use wiremock::MockServer;

/// Everything a Phase E/F test needs to assemble a
/// `StripeBillingProvider<PgOutboundRequestRepository>` against a real
/// Postgres ledger and a faked Stripe HTTP API.
///
/// Deliberately *not* a pre-assembled `StripeBillingProvider`: that type is
/// this crate's own Task 15 deliverable, and this harness (Task 14) has no
/// dependency on it. Handing back the ingredients -- a ready repository, the
/// pool for raw-SQL test setup, the mock server, and a `StripeConfig`
/// already pointed at it -- lets each test assemble exactly the provider it
/// needs in one line, without this file needing to know that type's shape.
///
/// Keep `TestEnv` alive for the duration of the test: the Postgres
/// container is torn down (via `Drop`) as soon as it goes out of scope, and
/// the mock server stops accepting connections the same way.
pub struct TestEnv {
    /// A repository over the disposable Postgres, ready to hand to a
    /// `Ledger` or `StripeBillingProvider`.
    pub repo: PgOutboundRequestRepository,
    /// The same pool `repo` is built on, for raw-SQL test setup (see
    /// `backdate`) and direct assertions against `billing.outbound_requests`.
    pub pool: PgPool,
    /// The faked Stripe HTTP API. Tests mount `wiremock::Mock`s on this and
    /// assert on `received_requests()`.
    pub mock_server: MockServer,
    /// A `StripeConfig` with `base_url` already pointed at `mock_server` --
    /// every outbound call a client built from this reaches the mock, never
    /// the real Stripe API.
    pub stripe_config: StripeConfig,
    _container: ContainerAsync<GenericImage>,
}

/// Starts a fresh, disposable Postgres container with migrations applied
/// and a fresh `wiremock` server, and returns everything needed to build a
/// `StripeBillingProvider` pointed at both.
pub async fn setup() -> Result<TestEnv, Box<dyn Error>> {
    let container = GenericImage::new("postgres", "16-alpine")
        .with_wait_for(WaitFor::log(
            LogWaitStrategy::stdout_or_stderr("database system is ready to accept connections")
                .with_times(2),
        ))
        .with_exposed_port(5432.tcp())
        .with_env_var("POSTGRES_PASSWORD", "postgres")
        .start()
        .await?;

    let port = container.get_host_port_ipv4(5432.tcp()).await?;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

    let pool = PgPoolOptions::new().connect(&url).await?;
    persistence::run_migrations(&pool).await?;

    let repo = PgOutboundRequestRepository::new(pool.clone());
    let mock_server = MockServer::start().await;
    let stripe_config = StripeConfig {
        secret: SecretString::from("sk_test_fake".to_string()),
        base_url: Some(mock_server.uri()),
    };

    Ok(TestEnv {
        repo,
        pool,
        mock_server,
        stripe_config,
        _container: container,
    })
}

/// Backdates a ledger row's `created_at`, so a test can simulate a row
/// whose idempotency key has aged past the 23-hour window (the reserve
/// state machine's cases D and E) without waiting 23 real hours.
///
/// Raw SQL rather than a `Ledger`/`OutboundRequestRepository` method:
/// backdating a row is not a capability production code should ever have
/// (a real ledger row's `created_at` is exactly when the attempt was
/// recorded, and nothing should be able to lie about that) -- it exists
/// only here, in test setup.
pub async fn backdate(
    pool: &PgPool,
    id: OutboundRequestId,
    hours_ago: i32,
) -> Result<(), Box<dyn Error>> {
    sqlx::query(
        "UPDATE billing.outbound_requests \
         SET created_at = now() - make_interval(hours => $2) \
         WHERE id = $1",
    )
    .bind(id.as_uuid())
    .bind(hours_ago)
    .execute(pool)
    .await?;
    Ok(())
}
