//! Account-level audit events: one row per active membership, in the
//! business write's own transaction.

mod common;

use std::collections::BTreeSet;
use std::error::Error;

use audit::{Action, Actor, CorrelationId, SubjectId, Target, TargetId};
use common::{membership, tenant, user};
use identity_domain::{AccountEvent, DisplayName, UserId, UserRepository};
use identity_pg::PgUserRepository;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

fn user_updated(user: UserId, correlation: Uuid) -> AccountEvent {
    AccountEvent {
        actor: Actor::User(SubjectId::new(user.as_uuid())),
        action: Action::UserUpdated,
        target: Target::User(TargetId::new(user.as_uuid())),
        occurred_at: OffsetDateTime::now_utc(),
        correlation_id: CorrelationId::new(correlation),
    }
}

async fn display_name(pool: &PgPool, user: UserId) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SELECT display_name FROM identity.users WHERE id = $1")
        .bind(user.as_uuid())
        .fetch_one(pool)
        .await
}

#[tokio::test]
async fn one_row_per_active_membership_sharing_one_correlation_id() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let users = PgUserRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    let tenant_b = tenant(&db.pool, "Tenant B").await?;
    let tenant_c = tenant(&db.pool, "Tenant C").await?;
    membership(&db.pool, alice, tenant_a, "owner", "active").await?;
    membership(&db.pool, alice, tenant_b, "member", "active").await?;
    membership(&db.pool, alice, tenant_c, "member", "suspended").await?;
    let correlation = Uuid::new_v4();

    users
        .update_display_name(
            alice,
            &DisplayName::parse("Alice L.")?,
            &user_updated(alice, correlation),
        )
        .await?;

    let rows: Vec<(Uuid, String, String, Option<Uuid>, Uuid)> = sqlx::query_as(
        "SELECT tenant_id, action, actor_kind, target_id, correlation_id \
           FROM audit.audit_log",
    )
    .fetch_all(&db.pool)
    .await?;
    let tenants: BTreeSet<Uuid> = rows.iter().map(|row| row.0).collect();

    assert_eq!(rows.len(), 2);
    assert_eq!(
        tenants,
        [tenant_a.as_uuid(), tenant_b.as_uuid()]
            .into_iter()
            .collect()
    );
    assert!(
        rows.iter()
            .all(|(_, action, actor, target, row_correlation)| {
                action == "user.updated"
                    && actor == "user"
                    && *target == Some(alice.as_uuid())
                    && *row_correlation == correlation
            })
    );
    assert_eq!(display_name(&db.pool, alice).await?, "Alice L.");
    Ok(())
}

/// The named limit of fanning out: no active membership, no row. The change
/// itself still commits.
#[tokio::test]
async fn a_user_with_no_active_membership_gets_no_row_but_the_change_commits()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let users = PgUserRepository::new(db.pool.clone());
    let nora = user(&db.pool, "nora@example.test", None).await?;

    users
        .update_display_name(
            nora,
            &DisplayName::parse("Nora")?,
            &user_updated(nora, Uuid::new_v4()),
        )
        .await?;

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM audit.audit_log")
        .fetch_one(&db.pool)
        .await?;
    assert_eq!(rows, 0);
    assert_eq!(display_name(&db.pool, nora).await?, "Nora");
    Ok(())
}

#[tokio::test]
async fn a_failed_audit_insert_leaves_neither_the_change_nor_any_row() -> Result<(), Box<dyn Error>>
{
    let db = common::setup().await?;
    let users = PgUserRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    let tenant_b = tenant(&db.pool, "Tenant B").await?;
    membership(&db.pool, alice, tenant_a, "owner", "active").await?;
    membership(&db.pool, alice, tenant_b, "member", "active").await?;

    // Fails the *second* audit insert only, so the test proves the first
    // one is rolled back with everything else. The interpolated uuid is one
    // this test generated.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE OR REPLACE FUNCTION audit.fail_second() RETURNS trigger AS $$
         BEGIN
           IF NEW.tenant_id = '{}' THEN
             RAISE EXCEPTION 'test-forced audit insert failure';
           END IF;
           RETURN NEW;
         END;
         $$ LANGUAGE plpgsql;
         CREATE TRIGGER fail_second_trigger
           BEFORE INSERT ON audit.audit_log
           FOR EACH ROW EXECUTE FUNCTION audit.fail_second();",
        tenant_a.as_uuid().max(tenant_b.as_uuid())
    )))
    .execute(&db.pool)
    .await?;

    let result = users
        .update_display_name(
            alice,
            &DisplayName::parse("Mallory")?,
            &user_updated(alice, Uuid::new_v4()),
        )
        .await;

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM audit.audit_log")
        .fetch_one(&db.pool)
        .await?;
    assert!(result.is_err());
    assert_eq!(rows, 0);
    assert_eq!(display_name(&db.pool, alice).await?, "Alice");
    Ok(())
}
