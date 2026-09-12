// Each file under `tests/` compiles this module into its own binary, and no
// single binary uses every helper here -- `sink.rs` doesn't need `url_as`,
// `shared_database.rs` builds its own pool from scratch. Not dead code --
// it's shared setup.
#![allow(dead_code)]

use std::error::Error;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use testcontainers::core::wait::LogWaitStrategy;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// A disposable Postgres container with `audit-pg`'s migrations already
/// applied.
///
/// Keep this alive for the duration of the test: the container is torn
/// down (via `Drop`) as soon as it goes out of scope. Mirrors
/// `persistence`'s own `tests/common/mod.rs` harness.
pub struct TestDb {
    pub pool: PgPool,
    port: u16,
    _container: ContainerAsync<GenericImage>,
}

impl TestDb {
    /// A connection URL to the same database, as a different role --
    /// used to connect as a restricted role rather than the superuser
    /// `pool` holds, so a grant can be tested as the role it actually
    /// applies to.
    pub fn url_as(&self, user: &str, password: &str) -> String {
        let port = self.port;
        format!("postgres://{user}:{password}@127.0.0.1:{port}/postgres")
    }
}

/// Starts a fresh Postgres container, connects as the superuser, and
/// applies `audit-pg`'s migrations.
pub async fn setup() -> Result<TestDb, Box<dyn Error>> {
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
    audit_pg::run_migrations(&pool).await?;

    Ok(TestDb {
        pool,
        port,
        _container: container,
    })
}
