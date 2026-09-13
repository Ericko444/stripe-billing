//! Proves the claim `run_migrations`'s own rustdoc makes: this crate's
//! migrator and `persistence::run_migrations` can run against **one**
//! database without colliding, because this one tracks its state in
//! `audit._sqlx_migrations` rather than the default `_sqlx_migrations`
//! `persistence` uses.
//!
//! Not exercised anywhere else: `persistence`'s own tests and `audit-pg`'s
//! `append_only` test each spin up their *own* container and run only their
//! own migrator, so neither ever puts both migration sets on one database.

use std::error::Error;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use testcontainers::core::wait::LogWaitStrategy;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

async fn fresh_pool() -> Result<(PgPool, ContainerAsync<GenericImage>), Box<dyn Error>> {
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
    Ok((pool, container))
}

#[tokio::test]
async fn both_migrators_share_one_database_and_are_idempotent() -> Result<(), Box<dyn Error>> {
    let (pool, _container) = fresh_pool().await?;

    // audit first, then billing -- and each runs twice, proving "safe to
    // call repeatedly" for both, on a database the other has already
    // touched.
    audit_pg::run_migrations(&pool).await?;
    persistence::run_migrations(&pool).await?;
    audit_pg::run_migrations(&pool).await?;
    persistence::run_migrations(&pool).await?;

    // Each migrator's own tracking table exists, is distinct, and has rows
    // -- proof neither one's migrations were skipped or misfiled into the
    // other's table.
    let audit_migrations: i64 = sqlx::query_scalar("SELECT count(*) FROM audit._sqlx_migrations")
        .fetch_one(&pool)
        .await?;
    assert!(
        audit_migrations > 0,
        "audit's migrator must have recorded its own migrations"
    );

    let billing_migrations: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await?;
    assert!(
        billing_migrations > 0,
        "the billing module's migrator must have recorded its own migrations"
    );

    // Both schemas' tables exist on the one shared database.
    sqlx::query("SELECT 1 FROM audit.audit_log LIMIT 0")
        .fetch_optional(&pool)
        .await?;
    sqlx::query("SELECT 1 FROM billing.customers LIMIT 0")
        .fetch_optional(&pool)
        .await?;

    Ok(())
}

#[tokio::test]
async fn migrator_order_does_not_matter() -> Result<(), Box<dyn Error>> {
    let (pool, _container) = fresh_pool().await?;

    // Reversed order from the test above -- billing first this time.
    persistence::run_migrations(&pool).await?;
    audit_pg::run_migrations(&pool).await?;

    sqlx::query("SELECT 1 FROM audit.audit_log LIMIT 0")
        .fetch_optional(&pool)
        .await?;
    sqlx::query("SELECT 1 FROM billing.customers LIMIT 0")
        .fetch_optional(&pool)
        .await?;

    Ok(())
}
