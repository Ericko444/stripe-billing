//! All three migrators on one database: the billing module's
//! (`_sqlx_migrations`), the audit journal's (`audit._sqlx_migrations`) and
//! the identity module's (`identity._sqlx_migrations`).
//!
//! Lives in `demo` because `demo` is the one crate allowed to name every
//! module -- `identity-pg` may not depend on `persistence`, even for a test,
//! and `persistence` may not depend on `identity-pg`.

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

async fn assert_all_three_present(pool: &PgPool) -> Result<(), Box<dyn Error>> {
    for tracking in [
        "_sqlx_migrations",
        "audit._sqlx_migrations",
        "identity._sqlx_migrations",
    ] {
        // `tracking` and `table` are literals from these two arrays, never
        // external input -- the `AssertSqlSafe` sqlx 0.9 requires for a
        // dynamic string.
        let applied: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM {tracking}"
        )))
        .fetch_one(pool)
        .await?;
        assert!(applied > 0, "{tracking} recorded nothing");
    }
    for table in ["billing.customers", "audit.audit_log", "identity.users"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT 1 FROM {table} LIMIT 0"
        )))
        .fetch_optional(pool)
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn three_migrators_share_one_database_and_are_idempotent() -> Result<(), Box<dyn Error>> {
    let (pool, _container) = fresh_pool().await?;

    for _ in 0..2 {
        persistence::run_migrations(&pool).await?;
        audit_pg::run_migrations(&pool).await?;
        identity_pg::run_migrations(&pool).await?;
    }

    assert_all_three_present(&pool).await
}

#[tokio::test]
async fn migrator_order_does_not_matter() -> Result<(), Box<dyn Error>> {
    let (pool, _container) = fresh_pool().await?;

    identity_pg::run_migrations(&pool).await?;
    audit_pg::run_migrations(&pool).await?;
    persistence::run_migrations(&pool).await?;

    assert_all_three_present(&pool).await
}
