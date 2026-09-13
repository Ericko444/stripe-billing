use audit::AuditEntry;
use identity_domain::{AccountEvent, RepositoryError, UserId};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::repository_error;

/// Writes `event` once for every tenant `user_id` is an active member of, on
/// the caller's transaction, and returns how many rows it wrote.
///
/// The memberships are read inside the same transaction and locked
/// `FOR SHARE`: a suspension racing this write waits for it to commit, so
/// the tenants recorded are exactly the ones the user belonged to when the
/// change it describes became true. Built once and shared by every
/// account-level write -- profile update, password change, password reset.
pub(crate) async fn record_for_each_tenant(
    tx: &mut PgConnection,
    user_id: UserId,
    event: &AccountEvent,
) -> Result<usize, RepositoryError> {
    let tenants: Vec<Uuid> = sqlx::query_scalar(
        "SELECT tenant_id FROM identity.memberships \
          WHERE user_id = $1 AND status = 'active' \
          ORDER BY tenant_id \
          FOR SHARE",
    )
    .bind(user_id.as_uuid())
    .fetch_all(&mut *tx)
    .await
    .map_err(repository_error)?;

    for tenant in &tenants {
        let entry = AuditEntry::new(
            audit::TenantId::new(*tenant),
            event.actor,
            event.action,
            event.target,
            event.occurred_at,
            event.correlation_id,
        );
        audit_pg::insert(&mut *tx, &entry)
            .await
            .map_err(repository_error)?;
    }
    Ok(tenants.len())
}
