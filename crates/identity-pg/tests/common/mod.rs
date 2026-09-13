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

/// Inserts a tenant named `name`.
pub async fn tenant(pool: &PgPool, name: &str) -> Result<identity_domain::TenantId, sqlx::Error> {
    let id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO identity.tenants (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(identity_domain::TenantId::new(id))
}

/// Inserts a user with `email` and an optional stored hash.
pub async fn user(
    pool: &PgPool,
    email: &str,
    hash: Option<&str>,
) -> Result<identity_domain::UserId, sqlx::Error> {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO identity.users (id, email_normalized, display_name, password_hash) \
         VALUES ($1, $2, 'Alice', $3)",
    )
    .bind(id)
    .bind(email)
    .bind(hash)
    .execute(pool)
    .await?;
    Ok(identity_domain::UserId::new(id))
}

/// Inserts a membership with the given role and status spellings.
pub async fn membership(
    pool: &PgPool,
    user: identity_domain::UserId,
    tenant: identity_domain::TenantId,
    role: &str,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO identity.memberships (id, user_id, tenant_id, role, status) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(user.as_uuid())
    .bind(tenant.as_uuid())
    .bind(role)
    .bind(status)
    .execute(pool)
    .await?;
    Ok(())
}

/// Microsecond precision, matching `TIMESTAMPTZ`, so a round trip compares
/// equal.
pub fn now_micros() -> time::OffsetDateTime {
    let now = time::OffsetDateTime::now_utc();
    now.replace_nanosecond(now.nanosecond() / 1_000 * 1_000)
        .unwrap_or(now)
}

/// A session row for `token`, eight hours long from now.
pub fn session_for(
    token: &identity_domain::SplitToken,
    user: identity_domain::UserId,
    tenant: Option<identity_domain::TenantId>,
) -> identity_domain::NewSession {
    let authenticated_at = now_micros();
    identity_domain::NewSession {
        selector: token.selector(),
        verifier_hash: token.verifier().hash(),
        user_id: user,
        tenant_id: tenant,
        authenticated_at,
        expires_at: authenticated_at + time::Duration::hours(8),
    }
}
