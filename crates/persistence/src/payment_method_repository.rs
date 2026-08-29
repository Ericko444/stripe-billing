use domain::{
    CustomerId, DomainError, PaymentMethod, PaymentMethodId, PaymentMethodRepository, TenantId,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::RepositoryError;

#[derive(sqlx::FromRow)]
struct PaymentMethodRow {
    id: Uuid,
    tenant_id: Uuid,
    customer_id: Uuid,
    stripe_payment_method_id: String,
    brand: String,
    last4: String,
    is_default: bool,
    created_at: OffsetDateTime,
    deleted_at: Option<OffsetDateTime>,
}

impl From<PaymentMethodRow> for PaymentMethod {
    fn from(row: PaymentMethodRow) -> Self {
        PaymentMethod {
            id: PaymentMethodId::new(row.id),
            tenant_id: TenantId::new(row.tenant_id),
            customer_id: CustomerId::new(row.customer_id),
            stripe_payment_method_id: row.stripe_payment_method_id,
            brand: row.brand,
            last4: row.last4,
            is_default: row.is_default,
            created_at: row.created_at,
            deleted_at: row.deleted_at,
        }
    }
}

fn to_domain_error(err: RepositoryError) -> DomainError {
    DomainError::Repository(err.to_string())
}

/// Postgres-backed `PaymentMethodRepository`.
pub struct PgPaymentMethodRepository {
    pool: PgPool,
}

impl PgPaymentMethodRepository {
    /// Wraps a `PgPool` as a `PgPaymentMethodRepository`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl PaymentMethodRepository for PgPaymentMethodRepository {
    async fn create(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        stripe_payment_method_id: String,
        brand: String,
        last4: String,
        is_default: bool,
    ) -> Result<PaymentMethod, DomainError> {
        let row = sqlx::query_as::<_, PaymentMethodRow>(
            "INSERT INTO billing.payment_methods \
                 (id, tenant_id, customer_id, stripe_payment_method_id, brand, last4, is_default) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             RETURNING id, tenant_id, customer_id, stripe_payment_method_id, brand, last4, \
                       is_default, created_at, deleted_at",
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id.as_uuid())
        .bind(customer_id.as_uuid())
        .bind(stripe_payment_method_id)
        .bind(brand)
        .bind(last4)
        .bind(is_default)
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        Ok(row.into())
    }

    async fn find(
        &self,
        tenant_id: TenantId,
        id: PaymentMethodId,
    ) -> Result<Option<PaymentMethod>, DomainError> {
        let row = sqlx::query_as::<_, PaymentMethodRow>(
            "SELECT id, tenant_id, customer_id, stripe_payment_method_id, brand, last4, \
                    is_default, created_at, deleted_at \
             FROM billing.payment_methods \
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

    async fn list(&self, tenant_id: TenantId) -> Result<Vec<PaymentMethod>, DomainError> {
        let rows = sqlx::query_as::<_, PaymentMethodRow>(
            "SELECT id, tenant_id, customer_id, stripe_payment_method_id, brand, last4, \
                    is_default, created_at, deleted_at \
             FROM billing.payment_methods \
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
