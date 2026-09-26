use audit::{Action, Actor, AuditEntry, SubjectId, Target, TargetId};
use identity_domain::{
    Email, GrantOutcome, MemberGrant, MemberRepository, MemberSuspension, Membership, MembershipId,
    MembershipRepository, MembershipStatus, RepositoryError, Role, TenantId, TenantMember,
    TokenPurpose, UserId,
};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::repository_error;

/// `MembershipRepository` and `MemberRepository` over
/// `identity.memberships`.
#[derive(Clone)]
pub struct PgMembershipRepository {
    pool: PgPool,
}

impl PgMembershipRepository {
    /// A repository over `pool`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl MembershipRepository for PgMembershipRepository {
    async fn active_for_user(&self, user_id: UserId) -> Result<Vec<Membership>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT m.id, m.user_id, m.tenant_id, t.name AS tenant_name, m.role \
               FROM identity.memberships m \
               JOIN identity.tenants t ON t.id = m.tenant_id \
              WHERE m.user_id = $1 AND m.status = 'active' \
              ORDER BY t.name, t.id",
        )
        .bind(user_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(repository_error)?;

        rows.iter()
            .map(|row| {
                let role: String = row.try_get("role").map_err(repository_error)?;
                Ok(Membership {
                    id: MembershipId::new(row.try_get::<Uuid, _>("id").map_err(repository_error)?),
                    user_id: UserId::new(
                        row.try_get::<Uuid, _>("user_id")
                            .map_err(repository_error)?,
                    ),
                    tenant_id: TenantId::new(
                        row.try_get::<Uuid, _>("tenant_id")
                            .map_err(repository_error)?,
                    ),
                    tenant_name: row.try_get("tenant_name").map_err(repository_error)?,
                    role: role.parse::<Role>().map_err(repository_error)?,
                })
            })
            .collect()
    }
}

impl MemberRepository for PgMembershipRepository {
    async fn list(&self, tenant_id: TenantId) -> Result<Vec<TenantMember>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT m.id, m.user_id, u.email_normalized, m.role, m.status \
               FROM identity.memberships m \
               JOIN identity.users u ON u.id = m.user_id \
              WHERE m.tenant_id = $1 \
              ORDER BY u.email_normalized",
        )
        .bind(tenant_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(repository_error)?;

        rows.iter().map(member_from_row).collect()
    }

    async fn find(
        &self,
        tenant_id: TenantId,
        membership_id: MembershipId,
    ) -> Result<Option<TenantMember>, RepositoryError> {
        // The tenant is part of the lookup, not checked afterwards: another
        // tenant's membership is not found, by the same query that misses an
        // unknown id.
        let row = sqlx::query(
            "SELECT m.id, m.user_id, u.email_normalized, m.role, m.status \
               FROM identity.memberships m \
               JOIN identity.users u ON u.id = m.user_id \
              WHERE m.id = $1 AND m.tenant_id = $2",
        )
        .bind(membership_id.as_uuid())
        .bind(tenant_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(repository_error)?;
        row.as_ref().map(member_from_row).transpose()
    }

    async fn suspend(&self, suspension: &MemberSuspension) -> Result<bool, RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(repository_error)?;

        let suspended: Option<Uuid> = sqlx::query_scalar(
            "UPDATE identity.memberships SET status = 'suspended', updated_at = now() \
              WHERE id = $1 AND tenant_id = $2 AND status = 'active' \
              RETURNING user_id",
        )
        .bind(suspension.membership_id.as_uuid())
        .bind(suspension.tenant_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(repository_error)?;
        let Some(user_id) = suspended else {
            return Ok(false);
        };

        // The session lookup already refuses a suspended membership; deleting
        // the sessions as well means none survives to be honoured by a lookup
        // that forgot to check. Only this tenant's: the person keeps every
        // other tenant they belong to.
        sqlx::query("DELETE FROM identity.sessions WHERE user_id = $1 AND tenant_id = $2")
            .bind(user_id)
            .bind(suspension.tenant_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(repository_error)?;

        let entry = AuditEntry::new(
            audit::TenantId::new(suspension.tenant_id.as_uuid()),
            Actor::User(SubjectId::new(suspension.suspended_by.as_uuid())),
            Action::MembershipSuspended,
            Target::Membership(TargetId::new(suspension.membership_id.as_uuid())),
            suspension.occurred_at,
            suspension.correlation_id,
        );
        audit_pg::insert(&mut tx, &entry)
            .await
            .map_err(repository_error)?;

        tx.commit().await.map_err(repository_error)?;
        Ok(true)
    }

    async fn grant(&self, grant: &MemberGrant) -> Result<GrantOutcome, RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(repository_error)?;
        let tenant = audit::TenantId::new(grant.tenant_id.as_uuid());
        let actor = Actor::User(SubjectId::new(grant.granted_by.as_uuid()));
        let entry = |action, target| {
            AuditEntry::new(
                tenant,
                actor,
                action,
                target,
                grant.occurred_at,
                grant.correlation_id,
            )
        };

        // Create the account if the address has none. `ON CONFLICT DO
        // NOTHING` rather than look-then-insert: two tenants adding the same
        // new address at once end with one account, not a unique violation.
        let created: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO identity.users (id, email_normalized) VALUES ($1, $2) \
             ON CONFLICT (email_normalized) DO NOTHING \
             RETURNING id",
        )
        .bind(Uuid::new_v4())
        .bind(grant.email.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(repository_error)?;

        // Read back under `FOR SHARE`, whichever way the account came to
        // exist: a reset completing concurrently waits for this commit, so
        // "has no password" is still true when the invitation is stored.
        let row = sqlx::query(
            "SELECT id, password_hash IS NULL AS no_password FROM identity.users \
              WHERE email_normalized = $1 FOR SHARE",
        )
        .bind(grant.email.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(repository_error)?;
        let user_id: Uuid = row.try_get("id").map_err(repository_error)?;
        let no_password: bool = row.try_get("no_password").map_err(repository_error)?;

        let inserted: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO identity.memberships (id, user_id, tenant_id, role, status) \
             VALUES ($1, $2, $3, $4, 'active') \
             ON CONFLICT (user_id, tenant_id) DO NOTHING \
             RETURNING id",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(grant.tenant_id.as_uuid())
        .bind(grant.role.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(repository_error)?;
        let Some(membership_id) = inserted else {
            // Dropping the transaction rolls it back. An account created
            // above cannot be the one that conflicts, so nothing is lost.
            return Ok(GrantOutcome::AlreadyMember);
        };

        if created.is_some() {
            audit_pg::insert(
                &mut tx,
                &entry(Action::UserCreated, Target::User(TargetId::new(user_id))),
            )
            .await
            .map_err(repository_error)?;
        }
        audit_pg::insert(
            &mut tx,
            &entry(
                Action::MembershipGranted,
                Target::Membership(TargetId::new(membership_id)),
            ),
        )
        .await
        .map_err(repository_error)?;

        if no_password {
            let purpose = TokenPurpose::Invitation.as_str();
            sqlx::query("DELETE FROM identity.password_tokens WHERE user_id = $1 AND purpose = $2")
                .bind(user_id)
                .bind(purpose)
                .execute(&mut *tx)
                .await
                .map_err(repository_error)?;
            sqlx::query(
                "INSERT INTO identity.password_tokens \
                    (id, selector, verifier_hash, user_id, purpose, created_at, expires_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(Uuid::new_v4())
            .bind(&grant.invitation.selector.as_bytes()[..])
            .bind(&grant.invitation.verifier_hash.as_bytes()[..])
            .bind(user_id)
            .bind(purpose)
            .bind(grant.occurred_at)
            .bind(grant.invitation.expires_at)
            .execute(&mut *tx)
            .await
            .map_err(repository_error)?;
        }

        tx.commit().await.map_err(repository_error)?;
        Ok(GrantOutcome::Granted {
            member: TenantMember {
                membership_id: MembershipId::new(membership_id),
                user_id: UserId::new(user_id),
                email: grant.email.clone(),
                role: grant.role,
                status: MembershipStatus::Active,
            },
            invitation_issued: no_password,
        })
    }
}

fn member_from_row(row: &PgRow) -> Result<TenantMember, RepositoryError> {
    let email: String = row.try_get("email_normalized").map_err(repository_error)?;
    let role: String = row.try_get("role").map_err(repository_error)?;
    let status: String = row.try_get("status").map_err(repository_error)?;
    Ok(TenantMember {
        membership_id: MembershipId::new(row.try_get::<Uuid, _>("id").map_err(repository_error)?),
        user_id: UserId::new(
            row.try_get::<Uuid, _>("user_id")
                .map_err(repository_error)?,
        ),
        email: Email::parse(&email).map_err(repository_error)?,
        role: role.parse::<Role>().map_err(repository_error)?,
        status: status
            .parse::<MembershipStatus>()
            .map_err(repository_error)?,
    })
}
