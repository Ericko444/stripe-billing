//! Deactivating and reactivating an account, at the database: the
//! sole-tenant constraint, what deactivation takes with it, idempotence, and
//! the audit rows.

mod common;

use std::error::Error;

use audit::{Action, Actor, CorrelationId, SubjectId, Target, TargetId};
use common::{membership, session_for, tenant, user};
use identity_domain::{
    AccountEvent, DeactivateOutcome, SessionRepository, SplitToken, TenantId, UserId,
    UserRepository,
};
use identity_pg::{PgSessionRepository, PgUserRepository};
use sqlx::PgPool;
use uuid::Uuid;

fn event(action: Action, user: UserId, correlation_id: CorrelationId) -> AccountEvent {
    AccountEvent {
        actor: Actor::User(SubjectId::new(user.as_uuid())),
        action,
        target: Target::User(TargetId::new(user.as_uuid())),
        occurred_at: common::now_micros(),
        correlation_id,
    }
}

/// The two rows a deactivation writes, as the use case will pass them.
fn deactivation_events(user: UserId, correlation_id: CorrelationId) -> Vec<AccountEvent> {
    vec![
        event(Action::UserDeactivated, user, correlation_id),
        event(Action::SessionsRevoked, user, correlation_id),
    ]
}

async fn actions_for(
    pool: &PgPool,
    correlation_id: CorrelationId,
) -> Result<Vec<(Uuid, String)>, Box<dyn Error>> {
    let rows = sqlx::query_as(
        "SELECT tenant_id, action FROM audit.audit_log \
          WHERE correlation_id = $1 ORDER BY action",
    )
    .bind(correlation_id.as_uuid())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn deactivated_at(pool: &PgPool, user: UserId) -> Result<bool, Box<dyn Error>> {
    let set: bool =
        sqlx::query_scalar("SELECT deactivated_at IS NOT NULL FROM identity.users WHERE id = $1")
            .bind(user.as_uuid())
            .fetch_one(pool)
            .await?;
    Ok(set)
}

async fn session_count(pool: &PgPool, user: UserId) -> Result<i64, Box<dyn Error>> {
    let count = sqlx::query_scalar("SELECT count(*) FROM identity.sessions WHERE user_id = $1")
        .bind(user.as_uuid())
        .fetch_one(pool)
        .await?;
    Ok(count)
}

async fn token_count(pool: &PgPool, user: UserId) -> Result<i64, Box<dyn Error>> {
    let count =
        sqlx::query_scalar("SELECT count(*) FROM identity.password_tokens WHERE user_id = $1")
            .bind(user.as_uuid())
            .fetch_one(pool)
            .await?;
    Ok(count)
}

/// Deactivation ends the account everywhere: the flag is set, every session
/// in every tenant is gone, and so is any link that could set a password.
#[tokio::test]
async fn deactivating_clears_every_session_and_link_in_every_tenant() -> Result<(), Box<dyn Error>>
{
    let db = common::setup().await?;
    let users = PgUserRepository::new(db.pool.clone());
    let sessions = PgSessionRepository::new(db.pool.clone());

    let a = tenant(&db.pool, "Tenant A").await?;
    let b = tenant(&db.pool, "Tenant B").await?;
    let alice = user(&db.pool, "alice@example.test", Some("$hash")).await?;
    membership(&db.pool, alice, a, "owner", "active").await?;
    membership(&db.pool, alice, b, "member", "active").await?;

    // A session in each tenant, and an outstanding reset link.
    sessions
        .create(
            &session_for(&SplitToken::from_bytes([1; 16], [1; 32]), alice, Some(a)),
            None,
        )
        .await?;
    sessions
        .create(
            &session_for(&SplitToken::from_bytes([2; 16], [2; 32]), alice, Some(b)),
            None,
        )
        .await?;
    sqlx::query(
        "INSERT INTO identity.password_tokens \
            (id, selector, verifier_hash, user_id, purpose, created_at, expires_at) \
         VALUES ($1, $2, $3, $4, 'password_reset', now(), now() + interval '15 minutes')",
    )
    .bind(Uuid::new_v4())
    .bind(vec![9u8; 16])
    .bind(vec![9u8; 32])
    .bind(alice.as_uuid())
    .execute(&db.pool)
    .await?;

    let correlation_id = CorrelationId::new(Uuid::new_v4());
    // Self-service: no sole-tenant constraint, so belonging to two is fine.
    let outcome = users
        .deactivate(alice, None, &deactivation_events(alice, correlation_id))
        .await?;

    assert_eq!(outcome, DeactivateOutcome::Deactivated);
    assert!(deactivated_at(&db.pool, alice).await?);
    assert_eq!(session_count(&db.pool, alice).await?, 0);
    assert_eq!(token_count(&db.pool, alice).await?, 0);

    // One row per active membership, per event: two tenants x two events.
    let actions = actions_for(&db.pool, correlation_id).await?;
    assert_eq!(actions.len(), 4);
    for tenant_id in [a, b] {
        assert!(actions.contains(&(tenant_id.as_uuid(), "user.deactivated".into())));
        assert!(actions.contains(&(tenant_id.as_uuid(), "sessions.revoked".into())));
    }
    Ok(())
}

/// The sole-tenant constraint: an admin may end an account that lives
/// entirely inside their tenant, and may not reach one that also lives
/// elsewhere. Nothing is written in the refused case.
#[tokio::test]
async fn a_second_active_membership_refuses_a_tenant_scoped_deactivation()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let users = PgUserRepository::new(db.pool.clone());

    let a = tenant(&db.pool, "Tenant A").await?;
    let b = tenant(&db.pool, "Tenant B").await?;
    let alice = user(&db.pool, "alice@example.test", Some("$hash")).await?;
    membership(&db.pool, alice, a, "member", "active").await?;
    membership(&db.pool, alice, b, "member", "active").await?;

    let refused = CorrelationId::new(Uuid::new_v4());
    let outcome = users
        .deactivate(alice, Some(a), &deactivation_events(alice, refused))
        .await?;

    assert_eq!(outcome, DeactivateOutcome::BelongsToOtherTenants);
    assert!(!deactivated_at(&db.pool, alice).await?);
    assert!(actions_for(&db.pool, refused).await?.is_empty());

    Ok(())
}

/// A *suspended* membership elsewhere does not block it: the constraint is
/// about tenants the account is still active in.
#[tokio::test]
async fn a_suspended_membership_elsewhere_does_not_block_deactivation() -> Result<(), Box<dyn Error>>
{
    let db = common::setup().await?;
    let users = PgUserRepository::new(db.pool.clone());

    let a = tenant(&db.pool, "Tenant A").await?;
    let b = tenant(&db.pool, "Tenant B").await?;
    let alice = user(&db.pool, "alice@example.test", Some("$hash")).await?;
    membership(&db.pool, alice, a, "member", "active").await?;
    membership(&db.pool, alice, b, "member", "suspended").await?;

    let correlation_id = CorrelationId::new(Uuid::new_v4());
    let outcome = users
        .deactivate(alice, Some(a), &deactivation_events(alice, correlation_id))
        .await?;

    assert_eq!(outcome, DeactivateOutcome::Deactivated);
    // The suspended tenant gets no row: fan-out records active memberships.
    let actions = actions_for(&db.pool, correlation_id).await?;
    assert_eq!(actions.len(), 2);
    assert!(
        actions
            .iter()
            .all(|(tenant_id, _)| *tenant_id == a.as_uuid())
    );
    Ok(())
}

/// Deactivating twice is not an error and is not a second journal entry:
/// the state asked for is the state it is in. Same contract as suspending
/// an already-suspended membership.
#[tokio::test]
async fn deactivating_twice_writes_nothing_the_second_time() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let users = PgUserRepository::new(db.pool.clone());

    let a = tenant(&db.pool, "Tenant A").await?;
    let alice = user(&db.pool, "alice@example.test", Some("$hash")).await?;
    membership(&db.pool, alice, a, "member", "active").await?;

    users
        .deactivate(
            alice,
            Some(a),
            &deactivation_events(alice, CorrelationId::new(Uuid::new_v4())),
        )
        .await?;

    let again = CorrelationId::new(Uuid::new_v4());
    let outcome = users
        .deactivate(alice, Some(a), &deactivation_events(alice, again))
        .await?;

    assert_eq!(outcome, DeactivateOutcome::AlreadyDeactivated);
    assert!(actions_for(&db.pool, again).await?.is_empty());
    Ok(())
}

/// Reactivation clears the flag and records it; doing it to an active
/// account writes nothing.
#[tokio::test]
async fn reactivating_restores_the_account_and_is_idempotent() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let users = PgUserRepository::new(db.pool.clone());

    let a = tenant(&db.pool, "Tenant A").await?;
    let alice = user(&db.pool, "alice@example.test", Some("$hash")).await?;
    membership(&db.pool, alice, a, "member", "active").await?;
    users
        .deactivate(
            alice,
            Some(a),
            &deactivation_events(alice, CorrelationId::new(Uuid::new_v4())),
        )
        .await?;

    let restored = CorrelationId::new(Uuid::new_v4());
    let done = users
        .reactivate(alice, &event(Action::UserReactivated, alice, restored))
        .await?;

    assert!(done);
    assert!(!deactivated_at(&db.pool, alice).await?);
    assert_eq!(
        actions_for(&db.pool, restored).await?,
        vec![(a.as_uuid(), "user.reactivated".to_string())]
    );

    // Again, on an account that is already active: nothing happens.
    let noop = CorrelationId::new(Uuid::new_v4());
    let done_again = users
        .reactivate(alice, &event(Action::UserReactivated, alice, noop))
        .await?;
    assert!(!done_again);
    assert!(actions_for(&db.pool, noop).await?.is_empty());
    Ok(())
}

/// An account whose only tenant is the caller's is deactivable by that
/// tenant -- the case the constraint exists to allow.
#[tokio::test]
async fn a_sole_membership_is_deactivable_by_that_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let users = PgUserRepository::new(db.pool.clone());

    let a: TenantId = tenant(&db.pool, "Tenant A").await?;
    let carol = user(&db.pool, "carol@example.test", Some("$hash")).await?;
    membership(&db.pool, carol, a, "member", "active").await?;

    let correlation_id = CorrelationId::new(Uuid::new_v4());
    let outcome = users
        .deactivate(carol, Some(a), &deactivation_events(carol, correlation_id))
        .await?;

    assert_eq!(outcome, DeactivateOutcome::Deactivated);
    assert_eq!(actions_for(&db.pool, correlation_id).await?.len(), 2);
    Ok(())
}
