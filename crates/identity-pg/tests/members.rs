//! Adding members to a tenant and listing them, at the database -- and an
//! invitation link, redeemed.

mod common;

use std::error::Error;

use audit::CorrelationId;
use common::{membership, tenant, user};
use identity_domain::{
    Email, GrantOutcome, MemberGrant, MemberRepository, MembershipStatus, PasswordHash,
    PasswordTokenRepository, PendingInvitation, Role, SplitToken, TenantId, TokenPurpose, UserId,
};
use identity_pg::{PgMembershipRepository, PgPasswordTokenRepository};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

fn grant_of(
    tenant_id: TenantId,
    email: &str,
    role: Role,
    granted_by: UserId,
    invitation: &SplitToken,
) -> Result<MemberGrant, Box<dyn Error>> {
    let now = common::now_micros();
    Ok(MemberGrant {
        tenant_id,
        email: Email::parse(email)?,
        role,
        granted_by,
        invitation: PendingInvitation {
            selector: invitation.selector(),
            verifier_hash: invitation.verifier().hash(),
            expires_at: now + Duration::hours(72),
        },
        occurred_at: now,
        correlation_id: CorrelationId::new(Uuid::new_v4()),
    })
}

/// One audit row: action, tenant, actor subject, target kind, target id.
type AuditRow = (String, Uuid, Option<Uuid>, String, Option<Uuid>);

/// The audit rows written by one request, by action -- `audit_log` ids are
/// random, so there is no insertion order to read back.
async fn audit_rows(
    pool: &PgPool,
    correlation_id: CorrelationId,
) -> Result<Vec<AuditRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT action, tenant_id, actor_subject_id, target_kind, target_id \
           FROM audit.audit_log WHERE correlation_id = $1 ORDER BY action",
    )
    .bind(correlation_id.as_uuid())
    .fetch_all(pool)
    .await
}

async fn invitation_count(pool: &PgPool, email: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM identity.password_tokens t \
           JOIN identity.users u ON u.id = t.user_id \
          WHERE u.email_normalized = $1 AND t.purpose = 'invitation'",
    )
    .bind(email)
    .fetch_one(pool)
    .await
}

async fn stored_hash(pool: &PgPool, user: UserId) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT password_hash FROM identity.users WHERE id = $1")
        .bind(user.as_uuid())
        .fetch_one(pool)
        .await
}

#[tokio::test]
async fn adding_a_new_address_creates_a_passwordless_account_an_invitation_and_two_audit_rows()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let members = PgMembershipRepository::new(db.pool.clone());
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    let owner = user(&db.pool, "owner@example.test", Some("$owner")).await?;
    let invitation = SplitToken::from_bytes([1; 16], [1; 32]);
    let grant = grant_of(
        tenant_a,
        "carol@example.test",
        Role::Member,
        owner,
        &invitation,
    )?;

    let GrantOutcome::Granted {
        member,
        invitation_issued,
    } = members.grant(&grant).await?
    else {
        return Err("expected a grant".into());
    };

    assert!(invitation_issued);
    assert_eq!(member.email.as_str(), "carol@example.test");
    assert_eq!(member.status, MembershipStatus::Active);
    assert_eq!(stored_hash(&db.pool, member.user_id).await?, None);

    let stored = PgPasswordTokenRepository::new(db.pool.clone())
        .find(invitation.selector())
        .await?
        .ok_or("invitation not stored")?;
    assert_eq!(stored.purpose, TokenPurpose::Invitation);
    assert_eq!(stored.user_id, member.user_id);
    assert_eq!(stored.expires_at, grant.invitation.expires_at);

    assert_eq!(
        audit_rows(&db.pool, grant.correlation_id).await?,
        vec![
            (
                "membership.granted".to_string(),
                tenant_a.as_uuid(),
                Some(owner.as_uuid()),
                "membership".to_string(),
                Some(member.membership_id.as_uuid()),
            ),
            (
                "user.created".to_string(),
                tenant_a.as_uuid(),
                Some(owner.as_uuid()),
                "user".to_string(),
                Some(member.user_id.as_uuid()),
            ),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn adding_an_account_with_a_password_grants_only_the_membership() -> Result<(), Box<dyn Error>>
{
    let db = common::setup().await?;
    let members = PgMembershipRepository::new(db.pool.clone());
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    let tenant_b = tenant(&db.pool, "Tenant B").await?;
    let owner = user(&db.pool, "owner@example.test", Some("$owner")).await?;
    let bob = user(&db.pool, "bob@example.test", Some("$bob")).await?;
    membership(&db.pool, bob, tenant_b, "owner", "active").await?;
    let grant = grant_of(
        tenant_a,
        "bob@example.test",
        Role::Admin,
        owner,
        &SplitToken::from_bytes([2; 16], [2; 32]),
    )?;

    let GrantOutcome::Granted {
        member,
        invitation_issued,
    } = members.grant(&grant).await?
    else {
        return Err("expected a grant".into());
    };

    assert!(!invitation_issued);
    assert_eq!(member.user_id, bob);
    assert_eq!(member.role, Role::Admin);
    assert_eq!(invitation_count(&db.pool, "bob@example.test").await?, 0);
    let actions: Vec<String> = audit_rows(&db.pool, grant.correlation_id)
        .await?
        .into_iter()
        .map(|row| row.0)
        .collect();
    assert_eq!(actions, vec!["membership.granted".to_string()]);
    Ok(())
}

#[tokio::test]
async fn adding_an_existing_account_without_a_password_replaces_its_invitation()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let members = PgMembershipRepository::new(db.pool.clone());
    let tokens = PgPasswordTokenRepository::new(db.pool.clone());
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    let tenant_b = tenant(&db.pool, "Tenant B").await?;
    let owner = user(&db.pool, "owner@example.test", Some("$owner")).await?;
    let first = SplitToken::from_bytes([3; 16], [3; 32]);
    let second = SplitToken::from_bytes([4; 16], [4; 32]);

    members
        .grant(&grant_of(
            tenant_a,
            "carol@example.test",
            Role::Member,
            owner,
            &first,
        )?)
        .await?;
    let outcome = members
        .grant(&grant_of(
            tenant_b,
            "carol@example.test",
            Role::Member,
            owner,
            &second,
        )?)
        .await?;

    assert!(matches!(
        outcome,
        GrantOutcome::Granted {
            invitation_issued: true,
            ..
        }
    ));
    assert!(tokens.find(first.selector()).await?.is_none());
    assert!(tokens.find(second.selector()).await?.is_some());
    assert_eq!(invitation_count(&db.pool, "carol@example.test").await?, 1);
    Ok(())
}

#[tokio::test]
async fn adding_a_member_twice_changes_nothing_the_second_time() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let members = PgMembershipRepository::new(db.pool.clone());
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    let owner = user(&db.pool, "owner@example.test", Some("$owner")).await?;
    let bob = user(&db.pool, "bob@example.test", Some("$bob")).await?;
    membership(&db.pool, bob, tenant_a, "member", "suspended").await?;
    let grant = grant_of(
        tenant_a,
        "bob@example.test",
        Role::Admin,
        owner,
        &SplitToken::from_bytes([5; 16], [5; 32]),
    )?;

    assert_eq!(members.grant(&grant).await?, GrantOutcome::AlreadyMember);

    assert!(audit_rows(&db.pool, grant.correlation_id).await?.is_empty());
    let listed: Vec<(Role, MembershipStatus)> = members
        .list(tenant_a)
        .await?
        .iter()
        .map(|m| (m.role, m.status))
        .collect();
    assert_eq!(listed, vec![(Role::Member, MembershipStatus::Suspended)]);
    Ok(())
}

#[tokio::test]
async fn listing_members_shows_one_tenant_active_and_suspended_by_address()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let members = PgMembershipRepository::new(db.pool.clone());
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    let tenant_b = tenant(&db.pool, "Tenant B").await?;
    let zoe = user(&db.pool, "zoe@example.test", Some("$z")).await?;
    let adam = user(&db.pool, "adam@example.test", Some("$a")).await?;
    let other = user(&db.pool, "other@example.test", Some("$o")).await?;
    membership(&db.pool, zoe, tenant_a, "owner", "active").await?;
    membership(&db.pool, adam, tenant_a, "member", "suspended").await?;
    membership(&db.pool, other, tenant_b, "owner", "active").await?;

    let listed = members.list(tenant_a).await?;

    let summary: Vec<(&str, Role, MembershipStatus)> = listed
        .iter()
        .map(|m| (m.email.as_str(), m.role, m.status))
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "adam@example.test",
                Role::Member,
                MembershipStatus::Suspended
            ),
            ("zoe@example.test", Role::Owner, MembershipStatus::Active),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn an_invitation_sets_a_first_password_and_never_replaces_one() -> Result<(), Box<dyn Error>>
{
    let db = common::setup().await?;
    let members = PgMembershipRepository::new(db.pool.clone());
    let tokens = PgPasswordTokenRepository::new(db.pool.clone());
    let tenant_a = tenant(&db.pool, "Tenant A").await?;
    let owner = user(&db.pool, "owner@example.test", Some("$owner")).await?;
    let invitation = SplitToken::from_bytes([6; 16], [6; 32]);
    let grant = grant_of(
        tenant_a,
        "carol@example.test",
        Role::Member,
        owner,
        &invitation,
    )?;
    let GrantOutcome::Granted { member, .. } = members.grant(&grant).await? else {
        return Err("expected a grant".into());
    };
    let carol = member.user_id;
    let verifier_hash = invitation.verifier().hash();
    let new_hash = PasswordHash::new("$carol".to_string());
    let redeem_as = |purpose| {
        tokens.redeem(
            invitation.selector(),
            purpose,
            &verifier_hash,
            OffsetDateTime::now_utc(),
            &new_hash,
            &[],
        )
    };

    // An invitation is not a reset link.
    assert!(!redeem_as(TokenPurpose::PasswordReset).await?);

    // A password set some other way in the meantime wins over the link.
    sqlx::query("UPDATE identity.users SET password_hash = '$elsewhere' WHERE id = $1")
        .bind(carol.as_uuid())
        .execute(&db.pool)
        .await?;
    assert!(!redeem_as(TokenPurpose::Invitation).await?);
    assert_eq!(
        stored_hash(&db.pool, carol).await?.as_deref(),
        Some("$elsewhere")
    );
    assert!(tokens.find(invitation.selector()).await?.is_some());

    sqlx::query("UPDATE identity.users SET password_hash = NULL WHERE id = $1")
        .bind(carol.as_uuid())
        .execute(&db.pool)
        .await?;
    assert!(redeem_as(TokenPurpose::Invitation).await?);
    assert_eq!(
        stored_hash(&db.pool, carol).await?.as_deref(),
        Some("$carol")
    );
    assert!(tokens.find(invitation.selector()).await?.is_none());
    Ok(())
}
