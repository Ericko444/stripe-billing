//! The invariants `0001_identity.sql` enforces in Postgres itself, rather
//! than trusting every future repository method to respect them.

mod common;

use std::error::Error;

use sqlx::PgPool;
use uuid::Uuid;

const UNIQUE_VIOLATION: &str = "23505";
const FOREIGN_KEY_VIOLATION: &str = "23503";

async fn insert_user(pool: &PgPool, email: &str) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO identity.users (id, email_normalized) VALUES ($1, $2)")
        .bind(id)
        .bind(email)
        .execute(pool)
        .await?;
    Ok(id)
}

async fn insert_tenant(pool: &PgPool) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO identity.tenants (id, name) VALUES ($1, 'Tenant')")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(id)
}

async fn insert_token(pool: &PgPool, user_id: Uuid, purpose: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO identity.password_tokens \
            (id, selector, verifier_hash, user_id, purpose, created_at, expires_at) \
         VALUES ($1, $2, $3, $4, $5, now(), now() + interval '15 minutes')",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().as_bytes().to_vec())
    .bind([7u8; 32].to_vec())
    .bind(user_id)
    .bind(purpose)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_session(
    pool: &PgPool,
    user_id: Uuid,
    tenant_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO identity.sessions \
            (id, selector, verifier_hash, user_id, tenant_id, authenticated_at, expires_at) \
         VALUES ($1, $2, $3, $4, $5, now(), now() + interval '8 hours')",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().as_bytes().to_vec())
    .bind([7u8; 32].to_vec())
    .bind(user_id)
    .bind(tenant_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[tokio::test]
async fn migrations_are_idempotent_and_coexist_with_the_audit_migrator()
-> Result<(), Box<dyn Error>> {
    let db = common::fresh().await?;

    identity_pg::run_migrations(&db.pool).await?;
    audit_pg::run_migrations(&db.pool).await?;
    identity_pg::run_migrations(&db.pool).await?;
    audit_pg::run_migrations(&db.pool).await?;

    let identity_migrations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM identity._sqlx_migrations")
            .fetch_one(&db.pool)
            .await?;
    assert!(identity_migrations > 0);

    for table in [
        "tenants",
        "users",
        "memberships",
        "sessions",
        "password_tokens",
    ] {
        // `table` is one of five literals above, never external input.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT 1 FROM identity.{table} LIMIT 0"
        )))
        .fetch_optional(&db.pool)
        .await?;
    }
    sqlx::query("SELECT 1 FROM audit.audit_log LIMIT 0")
        .fetch_optional(&db.pool)
        .await?;

    Ok(())
}

/// The database half of R3: a second outstanding token of the same purpose
/// cannot exist while the first does, whatever order a repository issues
/// its statements in.
#[tokio::test]
async fn a_second_outstanding_token_of_the_same_purpose_is_rejected() -> Result<(), Box<dyn Error>>
{
    let db = common::setup().await?;
    let user = insert_user(&db.pool, "alice@example.test").await?;

    insert_token(&db.pool, user, "password_reset").await?;
    let second = insert_token(&db.pool, user, "password_reset").await;
    assert_eq!(
        second.as_ref().err().and_then(common::sqlstate).as_deref(),
        Some(UNIQUE_VIOLATION)
    );

    // A different purpose is a different slot.
    insert_token(&db.pool, user, "invitation").await?;

    Ok(())
}

#[tokio::test]
async fn one_address_is_one_account() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;

    insert_user(&db.pool, "alice@example.test").await?;
    let duplicate = insert_user(&db.pool, "alice@example.test").await;
    assert_eq!(
        duplicate
            .as_ref()
            .err()
            .and_then(common::sqlstate)
            .as_deref(),
        Some(UNIQUE_VIOLATION)
    );

    Ok(())
}

#[tokio::test]
async fn a_tenant_scoped_session_requires_a_membership() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let user = insert_user(&db.pool, "alice@example.test").await?;
    let tenant = insert_tenant(&db.pool).await?;

    let without_membership = insert_session(&db.pool, user, Some(tenant)).await;
    assert_eq!(
        without_membership
            .as_ref()
            .err()
            .and_then(common::sqlstate)
            .as_deref(),
        Some(FOREIGN_KEY_VIOLATION)
    );

    sqlx::query(
        "INSERT INTO identity.memberships (id, user_id, tenant_id, role, status) \
         VALUES ($1, $2, $3, 'member', 'active')",
    )
    .bind(Uuid::new_v4())
    .bind(user)
    .bind(tenant)
    .execute(&db.pool)
    .await?;
    insert_session(&db.pool, user, Some(tenant)).await?;

    // The tenant-less session, which exists only to pick a tenant, needs none.
    insert_session(&db.pool, user, None).await?;

    Ok(())
}
