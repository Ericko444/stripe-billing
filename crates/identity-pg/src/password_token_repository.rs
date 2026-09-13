use identity_domain::{AccountEvent, NewPasswordToken, PasswordTokenRepository, RepositoryError};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::repository_error;
use crate::fan_out::record_for_each_tenant;

/// `PasswordTokenRepository` over `identity.password_tokens`.
#[derive(Clone)]
pub struct PgPasswordTokenRepository {
    pool: PgPool,
}

impl PgPasswordTokenRepository {
    /// A repository over `pool`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl PasswordTokenRepository for PgPasswordTokenRepository {
    async fn replace(
        &self,
        token: &NewPasswordToken,
        event: Option<&AccountEvent>,
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(repository_error)?;

        // Supersede first, in the same transaction: the previous link stops
        // working at the exact commit that makes the new one work. Deleted,
        // not flagged -- a superseded token has no history worth keeping, and
        // the request that superseded it is in the audit journal.
        sqlx::query("DELETE FROM identity.password_tokens WHERE user_id = $1 AND purpose = $2")
            .bind(token.user_id.as_uuid())
            .bind(token.purpose.as_str())
            .execute(&mut *tx)
            .await
            .map_err(repository_error)?;
        sqlx::query(
            "INSERT INTO identity.password_tokens \
                (id, selector, verifier_hash, user_id, purpose, created_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(Uuid::new_v4())
        .bind(&token.selector.as_bytes()[..])
        .bind(&token.verifier_hash.as_bytes()[..])
        .bind(token.user_id.as_uuid())
        .bind(token.purpose.as_str())
        .bind(token.created_at)
        .bind(token.expires_at)
        .execute(&mut *tx)
        .await
        .map_err(repository_error)?;
        if let Some(event) = event {
            record_for_each_tenant(&mut tx, token.user_id, event).await?;
        }

        tx.commit().await.map_err(repository_error)?;
        Ok(())
    }
}
