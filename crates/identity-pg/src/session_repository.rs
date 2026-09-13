use audit::AuditEntry;
use identity_domain::{NewSession, RepositoryError, SessionId, SessionRepository};
use sqlx::PgPool;
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
}
