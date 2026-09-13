use identity_domain::{
    Membership, MembershipId, MembershipRepository, RepositoryError, Role, TenantId, UserId,
};
use sqlx::PgPool;
use sqlx::Row;
use uuid::Uuid;

use crate::error::repository_error;

/// `MembershipRepository` over `identity.memberships`.
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
