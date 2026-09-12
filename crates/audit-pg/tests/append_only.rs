//! Proves the append-only grant, not just that some connection can write.
//!
//! `common::setup` connects as the Postgres superuser, the same as
//! `persistence`'s test harness -- which is exactly why the assertions
//! below connect as a *different*, deliberately restricted role instead.
//! A superuser can always `UPDATE` or `DELETE`; asserting that would prove
//! nothing about `0001_audit_log.sql`'s grant. This test creates a
//! throwaway login role, grants it membership in `audit_writer` the way a
//! real deployment would grant its own application role, and connects as
//! that role for every statement below.

mod common;

use std::error::Error;

use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
async fn restricted_role_can_insert_but_not_update_or_delete() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;

    sqlx::query("CREATE ROLE audit_test_writer LOGIN PASSWORD 'audit_test_writer'")
        .execute(&db.pool)
        .await?;
    sqlx::query("GRANT audit_writer TO audit_test_writer")
        .execute(&db.pool)
        .await?;

    let restricted = PgPoolOptions::new()
        .connect(&db.url_as("audit_test_writer", "audit_test_writer"))
        .await?;

    let id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let correlation_id = Uuid::new_v4();

    // INSERT succeeds: the one thing this role exists to do.
    sqlx::query(
        "INSERT INTO audit.audit_log \
            (id, tenant_id, actor_kind, action, target_kind, occurred_at, correlation_id) \
         VALUES ($1, $2, 'system', 'test.seeded', 'test.fixture', now(), $3)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(correlation_id)
    .execute(&restricted)
    .await?;

    // SELECT succeeds: the role can read back what it wrote.
    let row_count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit.audit_log WHERE id = $1")
        .bind(id)
        .fetch_one(&restricted)
        .await?;
    assert_eq!(row_count, 1);

    // UPDATE is rejected: the grant does not include it.
    let update_result = sqlx::query("UPDATE audit.audit_log SET action = 'tampered' WHERE id = $1")
        .bind(id)
        .execute(&restricted)
        .await;
    assert!(
        update_result.is_err(),
        "the restricted role must not be able to UPDATE audit.audit_log"
    );

    // DELETE is rejected: the grant does not include it either -- this is
    // the append-only property itself, enforced at the database, not just
    // by `AuditSink` having no such method.
    let delete_result = sqlx::query("DELETE FROM audit.audit_log WHERE id = $1")
        .bind(id)
        .execute(&restricted)
        .await;
    assert!(
        delete_result.is_err(),
        "the restricted role must not be able to DELETE from audit.audit_log"
    );

    // The row seeded above is still there, untouched, proving the
    // rejected statements above did not partially apply.
    let row_count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit.audit_log WHERE id = $1")
        .bind(id)
        .fetch_one(&db.pool)
        .await?;
    assert_eq!(row_count, 1);

    Ok(())
}
