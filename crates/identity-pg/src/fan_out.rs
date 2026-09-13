use audit::AuditEntry;
use identity_domain::{AccountEvent, RepositoryError, UserId};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::repository_error;

/// Writes `event` once for every tenant `user_id` is an active member of, on
/// the caller's transaction, and returns how many rows it wrote.
///
/// # Why fan out
///
/// `audit.audit_log` requires a tenant on every row, and a password reset
/// has none: it happens to a person, not to a tenant. Each tenant the person
/// belongs to has a real interest in it -- their Owners should see that a
/// member's password was reset -- so the event is recorded in each of them,
/// with one correlation id tying the copies together.
///
/// The alternative was a `Scope { Tenant, Platform }` with a nullable
/// `tenant_id`: a migration on `audit_log`, a changed `AuditEntry::new`,
/// every billing call site touched, and tenant views that would have to
/// join against *current* memberships -- showing a tenant events from before
/// the person joined, and hiding events after they left. Fan-out records
/// the membership as it was when the change happened.
///
/// Two costs, named:
///
/// - **One fact is N rows.** Counting resets means counting distinct
///   correlation ids, not rows.
/// - **A user with no active membership gets no row** for an account-level
///   event: there is no tenant to record it under. The change itself still
///   happens; there is simply no journal it belongs to.
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
