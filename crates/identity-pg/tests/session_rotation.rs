//! `rotate` and `delete`: tenant selection and logout at the database.

mod common;

use std::error::Error;

use audit::{Action, Actor, AuditEntry, CorrelationId, SubjectId, Target, TargetId};
use common::{membership, session_for, tenant, user};
use identity_domain::{SessionRepository, SplitToken};
use identity_pg::PgSessionRepository;
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn rotation_replaces_the_old_session_and_audits_in_one_step() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let sessions = PgSessionRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    membership(&db.pool, alice, tenant_a, "owner", "active").await?;

    let old_token = SplitToken::from_bytes([1; 16], [2; 32]);
    let old = sessions
        .create(&session_for(&old_token, alice, None), None)
        .await?;
    let new_token = SplitToken::from_bytes([3; 16], [4; 32]);
    let correlation = Uuid::new_v4();
    let entry = AuditEntry::new(
        audit::TenantId::new(tenant_a.as_uuid()),
        Actor::User(SubjectId::new(alice.as_uuid())),
        Action::SessionStarted,
        Target::User(TargetId::new(alice.as_uuid())),
        OffsetDateTime::now_utc(),
        CorrelationId::new(correlation),
    );

    let rotated = sessions
        .rotate(
            old,
            &session_for(&new_token, alice, Some(tenant_a)),
            Some(&entry),
        )
        .await?;

    assert!(rotated.is_some());
    assert!(sessions.resolve(old_token.selector()).await?.is_none());
    assert!(sessions.resolve(new_token.selector()).await?.is_some());
    let audited: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit.audit_log WHERE correlation_id = $1")
            .bind(correlation)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(audited, 1);
    Ok(())
}

/// Two requests carrying the same token race to select a tenant. Only the
/// first rotation may leave a live session; the second finds its old row
/// gone and inserts nothing.
#[tokio::test]
async fn a_session_can_be_rotated_only_once() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let sessions = PgSessionRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let old = sessions
        .create(
            &session_for(&SplitToken::from_bytes([1; 16], [2; 32]), alice, None),
            None,
        )
        .await?;

    let first = sessions
        .rotate(
            old,
            &session_for(&SplitToken::from_bytes([3; 16], [4; 32]), alice, None),
            None,
        )
        .await?;
    let second = sessions
        .rotate(
            old,
            &session_for(&SplitToken::from_bytes([5; 16], [6; 32]), alice, None),
            None,
        )
        .await?;

    assert!(first.is_some());
    assert!(second.is_none());
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM identity.sessions")
        .fetch_one(&db.pool)
        .await?;
    assert_eq!(live, 1);
    Ok(())
}

#[tokio::test]
async fn delete_removes_the_session_and_is_idempotent() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let sessions = PgSessionRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let token = SplitToken::from_bytes([1; 16], [2; 32]);
    let id = sessions
        .create(&session_for(&token, alice, None), None)
        .await?;

    sessions.delete(id).await?;
    sessions.delete(id).await?;

    assert!(sessions.resolve(token.selector()).await?.is_none());
    Ok(())
}
