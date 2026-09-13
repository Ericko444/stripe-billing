//! `resolve`: the one query every authenticated request runs. The active-user
//! and active-membership rules live in its SQL, so these tests are the proof
//! that a suspension or deactivation is seen on the next request.

mod common;

use std::error::Error;

use common::{membership, session_for, tenant, user};
use identity_domain::{Role, SessionRepository, SessionTenant, SplitToken};
use identity_pg::PgSessionRepository;

#[tokio::test]
async fn a_tenant_scoped_session_resolves_with_its_role() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let sessions = PgSessionRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    membership(&db.pool, alice, tenant_a, "admin", "active").await?;
    let token = SplitToken::from_bytes([1; 16], [2; 32]);
    let new = session_for(&token, alice, Some(tenant_a));
    let id = sessions.create(&new, None).await?;

    let resolved = sessions.resolve(token.selector()).await?;

    assert!(matches!(
        &resolved,
        Some(s) if s.id == id
            && s.user_id == alice
            && s.tenant == Some(SessionTenant { tenant_id: tenant_a, role: Role::Admin })
            && s.verifier_hash == new.verifier_hash
            && s.authenticated_at == new.authenticated_at
            && s.expires_at == new.expires_at
    ));
    Ok(())
}

#[tokio::test]
async fn a_tenant_less_session_resolves_without_a_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let sessions = PgSessionRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let token = SplitToken::from_bytes([1; 16], [2; 32]);
    sessions
        .create(&session_for(&token, alice, None), None)
        .await?;

    let resolved = sessions.resolve(token.selector()).await?;
    assert!(matches!(&resolved, Some(s) if s.tenant.is_none()));
    Ok(())
}

#[tokio::test]
async fn an_unknown_selector_resolves_to_nothing() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let sessions = PgSessionRepository::new(db.pool.clone());

    let resolved = sessions
        .resolve(SplitToken::from_bytes([9; 16], [9; 32]).selector())
        .await?;
    assert!(resolved.is_none());
    Ok(())
}

#[tokio::test]
async fn a_suspended_membership_hides_its_session_but_not_the_users_others()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let sessions = PgSessionRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    let tenant_b = tenant(&db.pool, "Tenant B").await?;
    membership(&db.pool, alice, tenant_a, "member", "active").await?;
    membership(&db.pool, alice, tenant_b, "member", "active").await?;
    let in_a = SplitToken::from_bytes([1; 16], [2; 32]);
    let in_b = SplitToken::from_bytes([3; 16], [4; 32]);
    sessions
        .create(&session_for(&in_a, alice, Some(tenant_a)), None)
        .await?;
    sessions
        .create(&session_for(&in_b, alice, Some(tenant_b)), None)
        .await?;

    sqlx::query(
        "UPDATE identity.memberships SET status = 'suspended' WHERE user_id = $1 AND tenant_id = $2",
    )
    .bind(alice.as_uuid())
    .bind(tenant_a.as_uuid())
    .execute(&db.pool)
    .await?;

    assert!(sessions.resolve(in_a.selector()).await?.is_none());
    assert!(sessions.resolve(in_b.selector()).await?.is_some());
    Ok(())
}

#[tokio::test]
async fn a_deactivated_user_resolves_no_session_at_all() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let sessions = PgSessionRepository::new(db.pool.clone());
    let alice = user(&db.pool, "alice@example.test", None).await?;
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    membership(&db.pool, alice, tenant_a, "owner", "active").await?;
    let scoped = SplitToken::from_bytes([1; 16], [2; 32]);
    let unscoped = SplitToken::from_bytes([3; 16], [4; 32]);
    sessions
        .create(&session_for(&scoped, alice, Some(tenant_a)), None)
        .await?;
    sessions
        .create(&session_for(&unscoped, alice, None), None)
        .await?;

    sqlx::query("UPDATE identity.users SET deactivated_at = now() WHERE id = $1")
        .bind(alice.as_uuid())
        .execute(&db.pool)
        .await?;

    assert!(sessions.resolve(scoped.selector()).await?.is_none());
    assert!(sessions.resolve(unscoped.selector()).await?.is_none());
    Ok(())
}
