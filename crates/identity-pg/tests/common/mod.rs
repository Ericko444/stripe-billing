// Each file under `tests/` compiles this module into its own binary, and not
// every binary uses every helper here. Not dead code -- shared setup.
#![allow(dead_code)]

use std::error::Error;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use testcontainers::core::wait::LogWaitStrategy;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// A disposable Postgres container. Keep it alive for the whole test: the
/// container is torn down when this is dropped. Mirrors `audit-pg`'s and
/// `persistence`'s own harnesses.
pub struct TestDb {
    pub pool: PgPool,
    _container: ContainerAsync<GenericImage>,
}

/// Starts a fresh Postgres and connects as the superuser, applying no
/// migrations -- callers choose which migrators run, and in what order.
pub async fn fresh() -> Result<TestDb, Box<dyn Error>> {
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
    Ok(TestDb {
        pool,
        _container: container,
    })
}

/// A fresh Postgres with `identity-pg`'s and `audit-pg`'s migrations
/// applied -- the two schemas this crate's repositories write to.
pub async fn setup() -> Result<TestDb, Box<dyn Error>> {
    let db = fresh().await?;
    identity_pg::run_migrations(&db.pool).await?;
    audit_pg::run_migrations(&db.pool).await?;
    Ok(db)
}

/// The SQLSTATE of a failed statement, if it was a database error.
pub fn sqlstate(err: &sqlx::Error) -> Option<String> {
    err.as_database_error()
        .and_then(|db| db.code())
        .map(|code| code.into_owned())
}
