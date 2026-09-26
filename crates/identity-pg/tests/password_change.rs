//! An authenticated password change, at the database: one transaction that
//! swaps the hash, ends every other session everywhere, drops outstanding
//! links, and audits both facts once per tenant.

mod common;

use std::collections::BTreeMap;
use std::error::Error;

use audit::{Action, Actor, CorrelationId, SubjectId, Target, TargetId};
use common::{membership, session_for, tenant, user};
use identity_domain::{
    AccountEvent, PasswordHash, SessionRepository, SplitToken, UserId, UserRepository,
};
use identity_pg::{PgSessionRepository, PgUserRepository};
use time::OffsetDateTime;
use uuid::Uuid;

fn event(user: UserId, action: Action, correlation: Uuid) -> AccountEvent {
    AccountEvent {
        actor: Actor::User(SubjectId::new(user.as_uuid())),
        action,
        target: Target::User(TargetId::new(user.as_uuid())),
        occurred_at: OffsetDateTime::now_utc(),
        correlation_id: CorrelationId::new(correlation),
    }
}

#[tokio::test]
async fn a_change_ends_every_other_session_everywhere_and_audits_per_tenant()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let users = PgUserRepository::new(db.pool.clone());
    let sessions = PgSessionRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", Some("$old")).await?;
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    let tenant_b = tenant(&db.pool, "Tenant B").await?;
    membership(&db.pool, alice, tenant_a, "owner", "active").await?;
    membership(&db.pool, alice, tenant_b, "member", "active").await?;

    let current = SplitToken::from_bytes([1; 16], [1; 32]);
    let in_b = SplitToken::from_bytes([2; 16], [2; 32]);
    let unscoped = SplitToken::from_bytes([3; 16], [3; 32]);
    let kept = sessions
        .create(&session_for(&current, alice, Some(tenant_a)), None)
        .await?;
    sessions
        .create(&session_for(&in_b, alice, Some(tenant_b)), None)
        .await?;
    sessions
        .create(&session_for(&unscoped, alice, None), None)
        .await?;
    sqlx::query(
        "INSERT INTO identity.password_tokens \
            (id, selector, verifier_hash, user_id, purpose, created_at, expires_at) \
         VALUES ($1, $2, $3, $4, 'password_reset', now(), now() + interval '15 minutes')",
    )
    .bind(Uuid::new_v4())
    .bind([9u8; 16].to_vec())
    .bind([9u8; 32].to_vec())
    .bind(alice.as_uuid())
    .execute(&db.pool)
    .await?;
    let correlation = Uuid::new_v4();

    users
        .change_password(
            alice,
            &PasswordHash::new("$new".to_string()),
            kept,
            &[
                event(alice, Action::PasswordChanged, correlation),
                event(alice, Action::SessionsRevoked, correlation),
            ],
        )
        .await?;

    assert!(sessions.resolve(current.selector()).await?.is_some());
    assert!(sessions.resolve(in_b.selector()).await?.is_none());
    assert!(sessions.resolve(unscoped.selector()).await?.is_none());

    let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM identity.password_tokens")
        .fetch_one(&db.pool)
        .await?;
    assert_eq!(tokens, 0);

    let hash: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM identity.users WHERE id = $1")
            .bind(alice.as_uuid())
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(hash.as_deref(), Some("$new"));

    let rows: Vec<(String, Uuid)> =
        sqlx::query_as("SELECT action, tenant_id FROM audit.audit_log WHERE correlation_id = $1")
            .bind(correlation)
            .fetch_all(&db.pool)
            .await?;
    let mut per_action: BTreeMap<String, Vec<Uuid>> = BTreeMap::new();
    for (action, tenant) in rows {
        per_action.entry(action).or_default().push(tenant);
    }
    for tenants in per_action.values_mut() {
        tenants.sort();
    }
    let mut both = vec![tenant_a.as_uuid(), tenant_b.as_uuid()];
    both.sort();
    assert_eq!(
        per_action,
        BTreeMap::from([
            ("password.changed".to_string(), both.clone()),
            ("sessions.revoked".to_string(), both),
        ])
    );
    Ok(())
}
