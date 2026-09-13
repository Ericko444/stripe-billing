use audit::AuditEntry;
use identity_domain::{
    NewSession, RepositoryError, Role, Selector, SessionId, SessionRepository, SessionTenant,
    StoredSession, TenantId, UserId, VERIFIER_BYTES, VerifierHash,
};
use sqlx::PgPool;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::repository_error;

/// `SessionRepository` over `identity.sessions`.
#[derive(Clone)]
pub struct PgSessionRepository {
    pool: PgPool,
}

impl PgSessionRepository {
    /// A repository over `pool`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl SessionRepository for PgSessionRepository {
    async fn create(
        &self,
        session: &NewSession,
        audit: Option<&AuditEntry>,
    ) -> Result<SessionId, RepositoryError> {
        let id = Uuid::new_v4();

        // One transaction for the session and its audit row: a session that
        // exists without the record of it starting, or the reverse, is the
        // exact inconsistency the audit journal exists to rule out.
        let mut tx = self.pool.begin().await.map_err(repository_error)?;

        sqlx::query(
            "INSERT INTO identity.sessions \
                (id, selector, verifier_hash, user_id, tenant_id, authenticated_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(id)
        .bind(&session.selector.as_bytes()[..])
        .bind(&session.verifier_hash.as_bytes()[..])
        .bind(session.user_id.as_uuid())
        .bind(session.tenant_id.map(|tenant| tenant.as_uuid()))
        .bind(session.authenticated_at)
        .bind(session.expires_at)
        .execute(&mut *tx)
        .await
        .map_err(repository_error)?;

        if let Some(entry) = audit {
            audit_pg::insert(&mut tx, entry)
                .await
                .map_err(repository_error)?;
        }

        tx.commit().await.map_err(repository_error)?;
        Ok(SessionId::new(id))
    }

    async fn resolve(&self, selector: Selector) -> Result<Option<StoredSession>, RepositoryError> {
        // The active-user and active-membership conditions are in the query,
        // not in the caller, so there is no code path that reads a session
        // and forgets to check them. The LEFT JOIN plus the OR keeps the
        // tenant-less session (tenant_id IS NULL, no membership to join).
        let row = sqlx::query(
            "SELECT s.id, s.verifier_hash, s.user_id, s.tenant_id, s.authenticated_at, \
                    s.expires_at, m.role \
               FROM identity.sessions s \
               JOIN identity.users u \
                 ON u.id = s.user_id AND u.deactivated_at IS NULL \
               LEFT JOIN identity.memberships m \
                 ON m.user_id = s.user_id AND m.tenant_id = s.tenant_id \
              WHERE s.selector = $1 \
                AND (s.tenant_id IS NULL OR m.status = 'active')",
        )
        .bind(&selector.as_bytes()[..])
        .fetch_optional(&self.pool)
        .await
        .map_err(repository_error)?;

        let Some(row) = row else {
            return Ok(None);
        };

        let verifier_hash: Vec<u8> = row.try_get("verifier_hash").map_err(repository_error)?;
        let verifier_hash: [u8; VERIFIER_BYTES] = verifier_hash
            .try_into()
            .map_err(|_| RepositoryError("stored verifier hash has the wrong length".into()))?;
        let tenant_id: Option<Uuid> = row.try_get("tenant_id").map_err(repository_error)?;
        let role: Option<String> = row.try_get("role").map_err(repository_error)?;
        let tenant = match (tenant_id, role) {
            (Some(tenant_id), Some(role)) => Some(SessionTenant {
                tenant_id: TenantId::new(tenant_id),
                role: role.parse::<Role>().map_err(repository_error)?,
            }),
            (None, _) => None,
            (Some(_), None) => {
                return Err(RepositoryError(
                    "tenant-scoped session resolved without a membership".into(),
                ));
            }
        };

        Ok(Some(StoredSession {
            id: SessionId::new(row.try_get::<Uuid, _>("id").map_err(repository_error)?),
            verifier_hash: VerifierHash::from_bytes(verifier_hash),
            user_id: UserId::new(
                row.try_get::<Uuid, _>("user_id")
                    .map_err(repository_error)?,
            ),
            tenant,
            authenticated_at: row
                .try_get::<OffsetDateTime, _>("authenticated_at")
                .map_err(repository_error)?,
            expires_at: row
                .try_get::<OffsetDateTime, _>("expires_at")
                .map_err(repository_error)?,
        }))
    }
}
