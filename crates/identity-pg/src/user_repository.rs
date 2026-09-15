use identity_domain::{
    AccountEvent, DeactivateOutcome, DisplayName, Email, PasswordHash, RepositoryError, SessionId,
    TenantId, User, UserId, UserRepository,
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

    async fn deactivate(
        &self,
        user_id: UserId,
        sole_tenant: Option<TenantId>,
        events: &[AccountEvent],
    ) -> Result<DeactivateOutcome, RepositoryError> {
        // One transaction, for the reason `change_password` opens one: an
        // account marked closed whose sessions survived would be closed in
        // the journal and open in fact.
        let mut tx = self.pool.begin().await.map_err(repository_error)?;

        // The constraint, decided here rather than by the caller, so the
        // decision and the write it authorises are one transaction.
        //
        // `FOR SHARE`, like `fan_out`'s read, and counted in Rust because
        // Postgres refuses row locks on an aggregate. It locks the
        // memberships it finds, so one cannot be suspended between this
        // decision and the write -- which would otherwise flip a refusal
        // into a permission after the fact.
        //
        // What it does *not* do is stop a membership in a second tenant
        // being **inserted** concurrently: row locks bind existing rows, not
        // future ones. That race is left open deliberately -- closing it
        // needs `SERIALIZABLE` or a lock every invitation path also takes --
        // and its consequence is bounded and recoverable: the account is
        // deactivated as of a moment when this tenant *was* its only one,
        // and the tenant that just added it sees a deactivated member it can
        // reactivate. Nothing crosses a tenant boundary that was not already
        // being handed across one.
        if let Some(tenant) = sole_tenant {
            let others: Vec<Uuid> = sqlx::query_scalar(
                "SELECT tenant_id FROM identity.memberships \
                  WHERE user_id = $1 AND status = 'active' AND tenant_id <> $2 \
                    FOR SHARE",
            )
            .bind(user_id.as_uuid())
            .bind(tenant.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(repository_error)?;
            if !others.is_empty() {
                return Ok(DeactivateOutcome::BelongsToOtherTenants);
            }
        }

        // `IS NULL` in the predicate, not a read followed by a write: two
        // concurrent deactivations then settle in the database, and exactly
        // one of them reports having done it.
        let marked = sqlx::query(
            "UPDATE identity.users \
                SET deactivated_at = $2, updated_at = now() \
              WHERE id = $1 AND deactivated_at IS NULL",
        )
        .bind(user_id.as_uuid())
        .bind(occurred_at(events))
        .execute(&mut *tx)
        .await
        .map_err(repository_error)?
        .rows_affected();
        if marked == 0 {
            return Ok(DeactivateOutcome::AlreadyDeactivated);
        }

        sqlx::query("DELETE FROM identity.sessions WHERE user_id = $1")
            .bind(user_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(repository_error)?;
        // A link mailed before this would otherwise still set a password on
        // an account that is meant to be closed.
        sqlx::query("DELETE FROM identity.password_tokens WHERE user_id = $1")
            .bind(user_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(repository_error)?;
        for event in events {
            record_for_each_tenant(&mut tx, user_id, event).await?;
        }

        tx.commit().await.map_err(repository_error)?;
        Ok(DeactivateOutcome::Deactivated)
    }

    async fn reactivate(
        &self,
        user_id: UserId,
        event: &AccountEvent,
    ) -> Result<bool, RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(repository_error)?;

        let cleared = sqlx::query(
            "UPDATE identity.users \
                SET deactivated_at = NULL, updated_at = now() \
              WHERE id = $1 AND deactivated_at IS NOT NULL",
        )
        .bind(user_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(repository_error)?
        .rows_affected();
        if cleared == 0 {
            return Ok(false);
        }

        record_for_each_tenant(&mut tx, user_id, event).await?;
        tx.commit().await.map_err(repository_error)?;
        Ok(true)
    }
}

/// When the caller says this happened, for `deactivated_at`.
///
/// The events' own time, not `now()`: the column and the journal rows must
/// agree, and the caller's clock is what the journal records. An empty slice
/// cannot occur through the use cases, which always pass at least one event;
/// `now()` is the harmless fallback rather than a panic on a nominal path.
fn occurred_at(events: &[AccountEvent]) -> OffsetDateTime {
    events
        .first()
        .map(|event| event.occurred_at)
        .unwrap_or_else(OffsetDateTime::now_utc)
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
