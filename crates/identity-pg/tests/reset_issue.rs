//! Issuing reset links, at the database: R1 and R3.

mod common;

use std::error::Error;

use audit::{Action, Actor, CorrelationId, Target, TargetId};
use common::{membership, tenant, user};
use identity_domain::{
    AccountEvent, NewPasswordToken, PasswordTokenRepository, SplitToken, TokenPurpose, UserId,
};
use identity_pg::PgPasswordTokenRepository;
use secrecy::ExposeSecret;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

fn reset_token(token: &SplitToken, user: UserId) -> NewPasswordToken {
    let created_at = common::now_micros();
    NewPasswordToken {
        selector: token.selector(),
        verifier_hash: token.verifier().hash(),
        user_id: user,
        purpose: TokenPurpose::PasswordReset,
        created_at,
        expires_at: created_at + Duration::minutes(15),
    }
}

/// R1 -- a reset token is hashed at rest: nothing in the stored row can be
/// turned back into the link that was mailed.
#[tokio::test]
async fn stored_reset_token_contains_no_verifier_bytes() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let tokens = PgPasswordTokenRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", Some("$hash")).await?;
    let token = identity_service_free_token();

    tokens.replace(&reset_token(&token, alice), None).await?;

    let wire = token.to_wire().expose_secret().to_string();
    let verifier_hex = wire
        .split_once('.')
        .map(|(_, v)| v.to_string())
        .unwrap_or_default();
    let (selector, verifier_hash): (Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT selector, verifier_hash FROM identity.password_tokens WHERE user_id = $1",
    )
    .bind(alice.as_uuid())
    .fetch_one(&db.pool)
    .await?;

    assert_eq!(selector, token.selector().as_bytes().to_vec());
    assert_eq!(verifier_hash, token.verifier().hash().as_bytes().to_vec());
    assert_ne!(hex::encode(&verifier_hash), verifier_hex);
    // No column of the row holds the verifier, in any encoding.
    let row_text: String = sqlx::query_scalar(
        "SELECT row_to_json(t)::text FROM identity.password_tokens t WHERE user_id = $1",
    )
    .bind(alice.as_uuid())
    .fetch_one(&db.pool)
    .await?;
    assert!(!row_text.contains(&verifier_hex));
    Ok(())
}

/// R3 -- issuing a new reset token invalidates the previous one, in the same
/// commit.
#[tokio::test]
async fn issuing_a_reset_token_invalidates_the_previous_one() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let tokens = PgPasswordTokenRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", Some("$hash")).await?;
    let first = SplitToken::from_bytes([1; 16], [1; 32]);
    let second = SplitToken::from_bytes([2; 16], [2; 32]);

    tokens.replace(&reset_token(&first, alice), None).await?;
    tokens.replace(&reset_token(&second, alice), None).await?;

    let selectors: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT selector FROM identity.password_tokens WHERE user_id = $1 AND purpose = 'password_reset'",
    )
    .bind(alice.as_uuid())
    .fetch_all(&db.pool)
    .await?;
    assert_eq!(selectors, vec![second.selector().as_bytes().to_vec()]);
    Ok(())
}

#[tokio::test]
async fn a_reset_and_an_invitation_are_separate_slots() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let tokens = PgPasswordTokenRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let reset = SplitToken::from_bytes([1; 16], [1; 32]);
    let invitation = SplitToken::from_bytes([2; 16], [2; 32]);

    tokens.replace(&reset_token(&reset, alice), None).await?;
    let mut invite = reset_token(&invitation, alice);
    invite.purpose = TokenPurpose::Invitation;
    tokens.replace(&invite, None).await?;

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM identity.password_tokens WHERE user_id = $1")
            .bind(alice.as_uuid())
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(count, 2);
    Ok(())
}

#[tokio::test]
async fn issuing_records_the_request_once_per_tenant_as_anonymous() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let tokens = PgPasswordTokenRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", Some("$hash")).await?;
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    let tenant_b = tenant(&db.pool, "Tenant B").await?;
    membership(&db.pool, alice, tenant_a, "owner", "active").await?;
    membership(&db.pool, alice, tenant_b, "member", "active").await?;
    let correlation = Uuid::new_v4();
    let event = AccountEvent {
        actor: Actor::Anonymous,
        action: Action::PasswordResetRequested,
        target: Target::User(TargetId::new(alice.as_uuid())),
        occurred_at: OffsetDateTime::now_utc(),
        correlation_id: CorrelationId::new(correlation),
    };

    tokens
        .replace(
            &reset_token(&SplitToken::from_bytes([1; 16], [1; 32]), alice),
            Some(&event),
        )
        .await?;

    let rows: Vec<(String, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT action, actor_kind, actor_subject_id FROM audit.audit_log WHERE correlation_id = $1",
    )
    .bind(correlation)
    .fetch_all(&db.pool)
    .await?;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|(action, actor, subject)| {
        action == "password_reset.requested" && actor == "anonymous" && subject.is_none()
    }));
    Ok(())
}

/// A token built from fixed, distinctive bytes -- `identity-pg` does not
/// depend on the service crate's generator, and does not need to.
fn identity_service_free_token() -> SplitToken {
    SplitToken::from_bytes([0x5e; 16], [0xa7; 32])
}
