use identity_domain::{
    AccountEvent, DisplayName, Email, PasswordHash, RepositoryError, SessionId, User, UserId,
    UserRepository,
};
use sqlx::PgPool;
use sqlx::Row;
use sqlx::postgres::PgRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::repository_error;
use crate::fan_out::record_for_each_tenant;

/// `UserRepository` over `identity.users`.
#[derive(Clone)]
pub struct PgUserRepository {
    pool: PgPool,
}

impl PgUserRepository {
    /// A repository over `pool`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl UserRepository for PgUserRepository {
    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, email_normalized, display_name, password_hash, deactivated_at \
               FROM identity.users \
              WHERE email_normalized = $1",
        )
        .bind(email.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(repository_error)?;

        row.map(|row| user_from_row(&row)).transpose()
    }

    async fn find(&self, user_id: UserId) -> Result<Option<User>, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, email_normalized, display_name, password_hash, deactivated_at \
               FROM identity.users \
              WHERE id = $1",
        )
        .bind(user_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(repository_error)?;

        row.map(|row| user_from_row(&row)).transpose()
    }

    async fn rehash_password(
        &self,
        user_id: UserId,
        hash: &PasswordHash,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE identity.users SET password_hash = $2, updated_at = now() WHERE id = $1",
        )
        .bind(user_id.as_uuid())
        .bind(hash.as_str())
        .execute(&self.pool)
        .await
        .map_err(repository_error)?;
        Ok(())
    }

    async fn update_display_name(
        &self,
        user_id: UserId,
        display_name: &DisplayName,
        event: &AccountEvent,
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(repository_error)?;

        sqlx::query(
            "UPDATE identity.users SET display_name = $2, updated_at = now() WHERE id = $1",
        )
        .bind(user_id.as_uuid())
        .bind(display_name.as_str())
        .execute(&mut *tx)
        .await
        .map_err(repository_error)?;
        record_for_each_tenant(&mut tx, user_id, event).await?;

        tx.commit().await.map_err(repository_error)?;
        Ok(())
    }

    async fn change_password(
        &self,
        user_id: UserId,
        hash: &PasswordHash,
        keep: SessionId,
        events: &[AccountEvent],
    ) -> Result<(), RepositoryError> {
        // One transaction: if revoking the other sessions failed after the
        // hash changed, an attacker's session would survive a change made
        // precisely to end it -- while the audit log said it had ended.
        let mut tx = self.pool.begin().await.map_err(repository_error)?;

        sqlx::query(
            "UPDATE identity.users SET password_hash = $2, updated_at = now() WHERE id = $1",
        )
        .bind(user_id.as_uuid())
        .bind(hash.as_str())
        .execute(&mut *tx)
        .await
        .map_err(repository_error)?;
        sqlx::query("DELETE FROM identity.sessions WHERE user_id = $1 AND id <> $2")
            .bind(user_id.as_uuid())
            .bind(keep.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(repository_error)?;
        sqlx::query("DELETE FROM identity.password_tokens WHERE user_id = $1")
            .bind(user_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(repository_error)?;
        for event in events {
            record_for_each_tenant(&mut tx, user_id, event).await?;
        }

        tx.commit().await.map_err(repository_error)?;
        Ok(())
    }
}

fn user_from_row(row: &PgRow) -> Result<User, RepositoryError> {
    let email: String = row.try_get("email_normalized").map_err(repository_error)?;
    let password_hash: Option<String> = row.try_get("password_hash").map_err(repository_error)?;
    Ok(User {
        id: UserId::new(row.try_get::<Uuid, _>("id").map_err(repository_error)?),
        // A stored address that no longer parses is a data problem, not a
        // "no such user": surfacing it as an error keeps it out of the
        // unknown-address path.
        email: Email::parse(&email).map_err(repository_error)?,
        display_name: row.try_get("display_name").map_err(repository_error)?,
        password_hash: password_hash.map(PasswordHash::new),
        deactivated_at: row
            .try_get::<Option<OffsetDateTime>, _>("deactivated_at")
            .map_err(repository_error)?,
    })
}
