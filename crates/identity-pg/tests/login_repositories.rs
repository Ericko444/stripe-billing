//! The repositories login reads and writes through, against a real Postgres.

mod common;

use std::error::Error;

use audit::{Action, Actor, AuditEntry, CorrelationId, SubjectId, Target, TargetId};
use identity_domain::{
    Email, MembershipRepository, PasswordHash, Role, SessionRepository, SplitToken, TenantId,
    UserId, UserRepository,
};
use identity_pg::{PgMembershipRepository, PgSessionRepository, PgUserRepository};
use time::OffsetDateTime;
use uuid::Uuid;

use common::{membership, session_for, tenant, user};

fn session_started(user: UserId, tenant: TenantId, correlation: Uuid) -> AuditEntry {
    AuditEntry::new(
        audit::TenantId::new(tenant.as_uuid()),
        Actor::User(SubjectId::new(user.as_uuid())),
        Action::SessionStarted,
        Target::User(TargetId::new(user.as_uuid())),
        OffsetDateTime::now_utc(),
        CorrelationId::new(correlation),
    )
}

#[tokio::test]
async fn find_by_email_returns_the_stored_account_or_none() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let users = PgUserRepository::new(db.pool.clone());
    let id = user(&db.pool, "alice@example.test", Some("$argon2id$stored")).await?;

    let found = users
        .find_by_email(&Email::parse("ALICE@example.test")?)
        .await?;
    assert!(matches!(
        &found,
        Some(u) if u.id == id
            && u.password_hash.as_ref().map(PasswordHash::as_str) == Some("$argon2id$stored")
            && u.is_active()
    ));

    let missing = users
        .find_by_email(&Email::parse("nobody@example.test")?)
        .await?;
    assert!(missing.is_none());
    Ok(())
}

#[tokio::test]
async fn find_by_email_still_returns_a_deactivated_account() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let users = PgUserRepository::new(db.pool.clone());
    let id = user(&db.pool, "alice@example.test", Some("$argon2id$stored")).await?;
    sqlx::query("UPDATE identity.users SET deactivated_at = now() WHERE id = $1")
        .bind(id.as_uuid())
        .execute(&db.pool)
        .await?;

    let found = users
        .find_by_email(&Email::parse("alice@example.test")?)
        .await?;
    assert!(matches!(&found, Some(u) if !u.is_active()));
    Ok(())
}

#[tokio::test]
async fn rehash_replaces_the_stored_hash() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let users = PgUserRepository::new(db.pool.clone());
    let id = user(&db.pool, "alice@example.test", Some("$argon2id$old")).await?;

    users
        .rehash_password(id, &PasswordHash::new("$argon2id$new".to_string()))
        .await?;

    let stored: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM identity.users WHERE id = $1")
            .bind(id.as_uuid())
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(stored.as_deref(), Some("$argon2id$new"));
    Ok(())
}

#[tokio::test]
async fn active_for_user_skips_suspended_memberships_and_orders_by_tenant_name()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let memberships = PgMembershipRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let tenant_b = tenant(&db.pool, "Tenant B").await?;
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    let tenant_c = tenant(&db.pool, "Tenant C").await?;
    membership(&db.pool, alice, tenant_b, "member", "active").await?;
    membership(&db.pool, alice, tenant_a, "owner", "active").await?;
    membership(&db.pool, alice, tenant_c, "admin", "suspended").await?;

    let active = memberships.active_for_user(alice).await?;

    let summary: Vec<(TenantId, &str, Role)> = active
        .iter()
        .map(|m| (m.tenant_id, m.tenant_name.as_str(), m.role))
        .collect();
    assert_eq!(
        summary,
        vec![
            (tenant_a, "Tenant A", Role::Owner),
            (tenant_b, "Tenant B", Role::Member),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_stored_session_holds_the_verifier_hash_never_the_verifier() -> Result<(), Box<dyn Error>>
{
    let db = common::setup().await?;
    let sessions = PgSessionRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let token = SplitToken::from_bytes([1; 16], [2; 32]);

    let id = sessions
        .create(&session_for(&token, alice, None), None)
        .await?;

    let (selector, verifier_hash): (Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT selector, verifier_hash FROM identity.sessions WHERE id = $1")
            .bind(id.as_uuid())
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(selector, vec![1u8; 16]);
    assert_eq!(verifier_hash, token.verifier().hash().as_bytes().to_vec());
    assert_ne!(verifier_hash, vec![2u8; 32]);
    Ok(())
}

#[tokio::test]
async fn a_session_with_an_audit_entry_writes_both_and_one_without_writes_only_the_session()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let sessions = PgSessionRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    membership(&db.pool, alice, tenant_a, "owner", "active").await?;
    let correlation = Uuid::new_v4();

    sessions
        .create(
            &session_for(
                &SplitToken::from_bytes([1; 16], [2; 32]),
                alice,
                Some(tenant_a),
            ),
            Some(&session_started(alice, tenant_a, correlation)),
        )
        .await?;
    sessions
        .create(
            &session_for(&SplitToken::from_bytes([3; 16], [4; 32]), alice, None),
            None,
        )
        .await?;

    let sessions_stored: i64 = sqlx::query_scalar("SELECT count(*) FROM identity.sessions")
        .fetch_one(&db.pool)
        .await?;
    let audit_rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT tenant_id, action, target_kind FROM audit.audit_log WHERE correlation_id = $1",
    )
    .bind(correlation)
    .fetch_all(&db.pool)
    .await?;
    let all_audit_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM audit.audit_log")
        .fetch_one(&db.pool)
        .await?;

    assert_eq!(sessions_stored, 2);
    assert_eq!(
        audit_rows,
        vec![(
            tenant_a.as_uuid(),
            "session.started".to_string(),
            "user".to_string()
        )]
    );
    assert_eq!(all_audit_rows, 1);
    Ok(())
}

#[tokio::test]
async fn a_failed_audit_insert_leaves_no_session() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let sessions = PgSessionRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    membership(&db.pool, alice, tenant_a, "owner", "active").await?;

    // The target id is a uuid this test generated, never external input.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE OR REPLACE FUNCTION audit.fail_on_sentinel() RETURNS trigger AS $$
         BEGIN
           IF NEW.target_id = '{}' THEN
             RAISE EXCEPTION 'test-forced audit insert failure';
           END IF;
           RETURN NEW;
         END;
         $$ LANGUAGE plpgsql;
         CREATE TRIGGER fail_on_sentinel_trigger
           BEFORE INSERT ON audit.audit_log
           FOR EACH ROW EXECUTE FUNCTION audit.fail_on_sentinel();",
        alice.as_uuid()
    )))
    .execute(&db.pool)
    .await?;

    let result = sessions
        .create(
            &session_for(
                &SplitToken::from_bytes([1; 16], [2; 32]),
                alice,
                Some(tenant_a),
            ),
            Some(&session_started(alice, tenant_a, Uuid::new_v4())),
        )
        .await;

    let sessions_stored: i64 = sqlx::query_scalar("SELECT count(*) FROM identity.sessions")
        .fetch_one(&db.pool)
        .await?;
    assert!(result.is_err());
    assert_eq!(sessions_stored, 0);
    Ok(())
}
