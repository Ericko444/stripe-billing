//! Completing a password reset, at the database: R2, R4, R9, expiry, and the
//! audit rows the reviewer will look for.

mod common;

use std::collections::BTreeMap;
use std::error::Error;

use audit::{Action, Actor, CorrelationId, Target, TargetId};
use common::{membership, session_for, tenant, user};
use identity_domain::{
    AccountEvent, NewPasswordToken, PasswordHash, PasswordTokenRepository, SessionRepository,
    SplitToken, TokenPurpose, UserId,
};
use identity_pg::{PgPasswordTokenRepository, PgSessionRepository};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn issue(
    tokens: &PgPasswordTokenRepository,
    token: &SplitToken,
    user: UserId,
    expires_in: Duration,
) -> Result<(), Box<dyn Error>> {
    let expires_at = common::now_micros() + expires_in;
    tokens
        .replace(
            &NewPasswordToken {
                selector: token.selector(),
                verifier_hash: token.verifier().hash(),
                user_id: user,
                purpose: TokenPurpose::PasswordReset,
                created_at: expires_at - Duration::minutes(15),
                expires_at,
            },
            None,
        )
        .await?;
    Ok(())
}

fn completion_events(user: UserId, correlation: Uuid) -> Vec<AccountEvent> {
    [Action::PasswordResetCompleted, Action::SessionsRevoked]
        .into_iter()
        .map(|action| AccountEvent {
            actor: Actor::Anonymous,
            action,
            target: Target::User(TargetId::new(user.as_uuid())),
            occurred_at: OffsetDateTime::now_utc(),
            correlation_id: CorrelationId::new(correlation),
        })
        .collect()
}

async fn complete(
    tokens: &PgPasswordTokenRepository,
    token: &SplitToken,
    user: UserId,
    hash: &str,
) -> Result<bool, Box<dyn Error>> {
    Ok(tokens
        .redeem(
            token.selector(),
            TokenPurpose::PasswordReset,
            &token.verifier().hash(),
            OffsetDateTime::now_utc(),
            &PasswordHash::new(hash.to_string()),
            &completion_events(user, Uuid::new_v4()),
        )
        .await?)
}

async fn stored_hash(pool: &PgPool, user: UserId) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT password_hash FROM identity.users WHERE id = $1")
        .bind(user.as_uuid())
        .fetch_one(pool)
        .await
}

/// R2 -- a reset token works once.
#[tokio::test]
async fn a_reset_token_cannot_be_used_twice() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let tokens = PgPasswordTokenRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", Some("$old")).await?;
    let token = SplitToken::from_bytes([1; 16], [1; 32]);
    issue(&tokens, &token, alice, Duration::minutes(15)).await?;

    assert!(complete(&tokens, &token, alice, "$first").await?);
    assert!(!complete(&tokens, &token, alice, "$second").await?);

    assert_eq!(
        stored_hash(&db.pool, alice).await?.as_deref(),
        Some("$first")
    );
    assert!(tokens.find(token.selector()).await?.is_none());
    Ok(())
}

/// R4 -- two completions of one link racing each other: at most one
/// succeeds, and the password is the winner's, not a mix.
#[tokio::test]
async fn two_concurrent_completions_succeed_at_most_once() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let tokens = PgPasswordTokenRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", Some("$old")).await?;

    for round in 0..5u8 {
        let token = SplitToken::from_bytes([round; 16], [round; 32]);
        issue(&tokens, &token, alice, Duration::minutes(15)).await?;

        let (left, right) = tokio::join!(
            complete(&tokens, &token, alice, "$left"),
            complete(&tokens, &token, alice, "$right"),
        );
        let (left, right) = (left?, right?);

        assert!(left ^ right, "round {round}: left={left} right={right}");
        let expected = if left { "$left" } else { "$right" };
        assert_eq!(
            stored_hash(&db.pool, alice).await?.as_deref(),
            Some(expected)
        );
    }
    Ok(())
}

/// R9 -- completing a reset ends every session of the user, in every tenant
/// and outside any tenant, and records it once per tenant.
#[tokio::test]
async fn completing_a_reset_deletes_sessions_in_every_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let tokens = PgPasswordTokenRepository::new(db.pool.clone());
    let sessions = PgSessionRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", Some("$old")).await?;
    let bob = user(&db.pool, "bob@example.test", Some("$bob")).await?;
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    let tenant_b = tenant(&db.pool, "Tenant B").await?;
    membership(&db.pool, alice, tenant_a, "owner", "active").await?;
    membership(&db.pool, alice, tenant_b, "member", "active").await?;
    membership(&db.pool, bob, tenant_b, "owner", "active").await?;

    let in_a = SplitToken::from_bytes([1; 16], [1; 32]);
    let in_b = SplitToken::from_bytes([2; 16], [2; 32]);
    let unscoped = SplitToken::from_bytes([3; 16], [3; 32]);
    let bobs = SplitToken::from_bytes([4; 16], [4; 32]);
    sessions
        .create(&session_for(&in_a, alice, Some(tenant_a)), None)
        .await?;
    sessions
        .create(&session_for(&in_b, alice, Some(tenant_b)), None)
        .await?;
    sessions
        .create(&session_for(&unscoped, alice, None), None)
        .await?;
    sessions
        .create(&session_for(&bobs, bob, Some(tenant_b)), None)
        .await?;

    let token = SplitToken::from_bytes([9; 16], [9; 32]);
    issue(&tokens, &token, alice, Duration::minutes(15)).await?;
    let correlation = Uuid::new_v4();
    let completed = tokens
        .redeem(
            token.selector(),
            TokenPurpose::PasswordReset,
            &token.verifier().hash(),
            OffsetDateTime::now_utc(),
            &PasswordHash::new("$new".to_string()),
            &completion_events(alice, correlation),
        )
        .await?;

    assert!(completed);
    for gone in [&in_a, &in_b, &unscoped] {
        assert!(sessions.resolve(gone.selector()).await?.is_none());
    }
    // Another user's session in a shared tenant is untouched.
    assert!(sessions.resolve(bobs.selector()).await?.is_some());

    let rows: Vec<(String, Uuid, String)> = sqlx::query_as(
        "SELECT action, tenant_id, actor_kind FROM audit.audit_log WHERE correlation_id = $1",
    )
    .bind(correlation)
    .fetch_all(&db.pool)
    .await?;
    let mut per_action: BTreeMap<String, Vec<Uuid>> = BTreeMap::new();
    for (action, tenant, actor) in rows {
        assert_eq!(actor, "anonymous");
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
            ("password_reset.completed".to_string(), both.clone()),
            ("sessions.revoked".to_string(), both),
        ])
    );
    Ok(())
}

#[tokio::test]
async fn an_expired_or_mismatched_token_changes_nothing() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let tokens = PgPasswordTokenRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", Some("$old")).await?;

    let expired = SplitToken::from_bytes([1; 16], [1; 32]);
    issue(&tokens, &expired, alice, Duration::seconds(-1)).await?;
    assert!(!complete(&tokens, &expired, alice, "$new").await?);

    let token = SplitToken::from_bytes([2; 16], [2; 32]);
    issue(&tokens, &token, alice, Duration::minutes(15)).await?;
    let wrong_hash = SplitToken::from_bytes([2; 16], [7; 32]).verifier().hash();
    let completed = tokens
        .redeem(
            token.selector(),
            TokenPurpose::PasswordReset,
            &wrong_hash,
            OffsetDateTime::now_utc(),
            &PasswordHash::new("$new".to_string()),
            &completion_events(alice, Uuid::new_v4()),
        )
        .await?;
    assert!(!completed);

    assert_eq!(stored_hash(&db.pool, alice).await?.as_deref(), Some("$old"));
    assert!(tokens.find(token.selector()).await?.is_some());
    Ok(())
}
