use domain::{Customer, CustomerId, CustomerRepository, DomainError, TenantId};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::RepositoryError;

#[derive(sqlx::FromRow)]
struct CustomerRow {
    id: Uuid,
    tenant_id: Uuid,
    stripe_customer_id: Option<String>,
    created_at: OffsetDateTime,
    deleted_at: Option<OffsetDateTime>,
}

impl From<CustomerRow> for Customer {
    fn from(row: CustomerRow) -> Self {
        Customer {
            id: CustomerId::new(row.id),
            tenant_id: TenantId::new(row.tenant_id),
            stripe_customer_id: row.stripe_customer_id,
            created_at: row.created_at,
            deleted_at: row.deleted_at,
        }
    }
}

fn to_domain_error(err: RepositoryError) -> DomainError {
    DomainError::Repository(err.to_string())
}

/// Postgres-backed `CustomerRepository`.
pub struct PgCustomerRepository {
    pool: PgPool,
}

impl PgCustomerRepository {
    /// Wraps a `PgPool` as a `PgCustomerRepository`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl CustomerRepository for PgCustomerRepository {
    async fn create(
        &self,
        tenant_id: TenantId,
        stripe_customer_id: Option<String>,
    ) -> Result<Customer, DomainError> {
        let row = sqlx::query_as::<_, CustomerRow>(
            "INSERT INTO billing.customers (id, tenant_id, stripe_customer_id) \
             VALUES ($1, $2, $3) \
             RETURNING id, tenant_id, stripe_customer_id, created_at, deleted_at",
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id.as_uuid())
        .bind(stripe_customer_id)
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        Ok(row.into())
    }

    async fn find(
        &self,
        tenant_id: TenantId,
        id: CustomerId,
    ) -> Result<Option<Customer>, DomainError> {
        let row = sqlx::query_as::<_, CustomerRow>(
            "SELECT id, tenant_id, stripe_customer_id, created_at, deleted_at \
             FROM billing.customers \
             WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(tenant_id.as_uuid())
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        Ok(row.map(Into::into))
    }

    async fn list(&self, tenant_id: TenantId) -> Result<Vec<Customer>, DomainError> {
        let rows = sqlx::query_as::<_, CustomerRow>(
            "SELECT id, tenant_id, stripe_customer_id, created_at, deleted_at \
             FROM billing.customers \
             WHERE tenant_id = $1 AND deleted_at IS NULL",
        )
        .bind(tenant_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        Ok(rows.into_iter().map(Into::into).collect())
    }
}
