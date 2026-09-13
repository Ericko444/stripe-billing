use identity_domain::{
    AccountEvent, NewPasswordToken, PasswordHash, PasswordTokenRepository, RepositoryError,
    Selector, StoredPasswordToken, TokenPurpose, UserId, VERIFIER_BYTES, VerifierHash,
};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
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

    async fn find(
        &self,
        selector: Selector,
    ) -> Result<Option<StoredPasswordToken>, RepositoryError> {
        let row = sqlx::query(
            "SELECT user_id, purpose, verifier_hash, expires_at \
               FROM identity.password_tokens WHERE selector = $1",
        )
        .bind(&selector.as_bytes()[..])
        .fetch_optional(&self.pool)
        .await
        .map_err(repository_error)?;
        row.as_ref().map(stored_from_row).transpose()
    }

    async fn complete_reset(
        &self,
        selector: Selector,
        expected: &VerifierHash,
        now: OffsetDateTime,
        new_hash: &PasswordHash,
        events: &[AccountEvent],
    ) -> Result<bool, RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(repository_error)?;

        // The row lock is what makes a link single-use under concurrency: a
        // second completion of the same link blocks here until the first
        // commits, then finds no row and changes nothing.
        let row = sqlx::query(
            "SELECT user_id, purpose, verifier_hash, expires_at \
               FROM identity.password_tokens WHERE selector = $1 \
               FOR UPDATE",
        )
        .bind(&selector.as_bytes()[..])
        .fetch_optional(&mut *tx)
        .await
        .map_err(repository_error)?;
        let Some(stored) = row.as_ref().map(stored_from_row).transpose()? else {
            return Ok(false);
        };
        // Re-checked under the lock, not trusted from the caller's earlier
        // read: the row could have been superseded or expired in between.
        // The verifier was already compared in constant time by the caller;
        // this equality is between two *stored* hashes of one row.
        if &stored.verifier_hash != expected
            || stored.purpose != TokenPurpose::PasswordReset
            || stored.expires_at <= now
        {
            return Ok(false);
        }
        let user = stored.user_id.as_uuid();

        // Consume the link, set the password, end every session everywhere,
        // drop every other outstanding link -- one commit, or none. If the
        // sessions were revoked in a separate step and that step failed, an
        // attacker's session would outlive the reset while the audit rows
        // said it had ended.
        sqlx::query("DELETE FROM identity.password_tokens WHERE user_id = $1")
            .bind(user)
            .execute(&mut *tx)
            .await
            .map_err(repository_error)?;
        sqlx::query(
            "UPDATE identity.users SET password_hash = $2, updated_at = now() WHERE id = $1",
        )
        .bind(user)
        .bind(new_hash.as_str())
        .execute(&mut *tx)
        .await
        .map_err(repository_error)?;
        sqlx::query("DELETE FROM identity.sessions WHERE user_id = $1")
            .bind(user)
            .execute(&mut *tx)
            .await
            .map_err(repository_error)?;
        for event in events {
            record_for_each_tenant(&mut tx, stored.user_id, event).await?;
        }

        tx.commit().await.map_err(repository_error)?;
        Ok(true)
    }
}

fn stored_from_row(row: &PgRow) -> Result<StoredPasswordToken, RepositoryError> {
    let purpose: String = row.try_get("purpose").map_err(repository_error)?;
    let purpose = match purpose.as_str() {
        "password_reset" => TokenPurpose::PasswordReset,
        "invitation" => TokenPurpose::Invitation,
        other => return Err(RepositoryError(format!("unknown token purpose {other:?}"))),
    };
    let verifier_hash: Vec<u8> = row.try_get("verifier_hash").map_err(repository_error)?;
    let verifier_hash: [u8; VERIFIER_BYTES] = verifier_hash
        .try_into()
        .map_err(|_| RepositoryError("stored verifier hash has the wrong length".into()))?;
    Ok(StoredPasswordToken {
        user_id: UserId::new(
            row.try_get::<Uuid, _>("user_id")
                .map_err(repository_error)?,
        ),
        purpose,
        verifier_hash: VerifierHash::from_bytes(verifier_hash),
        expires_at: row
            .try_get::<OffsetDateTime, _>("expires_at")
            .map_err(repository_error)?,
    })
}
