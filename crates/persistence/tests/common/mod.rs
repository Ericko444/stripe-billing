use std::error::Error;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use testcontainers::core::wait::LogWaitStrategy;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// A disposable Postgres container with migrations already applied.
///
/// Keep this alive for the duration of the test: the container is torn
/// down (via `Drop`) as soon as it goes out of scope.
pub struct TestDb {
    pub pool: PgPool,
    _container: ContainerAsync<GenericImage>,
}

/// Starts a fresh Postgres container, connects to it, and applies migrations.
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
    persistence::run_migrations(&pool).await?;

    Ok(TestDb {
        pool,
        _container: container,
    })
}
