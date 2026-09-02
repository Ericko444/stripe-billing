use domain::{
    CustomerId, DomainError, EventApplication, PaymentMethod, PaymentMethodId,
    PaymentMethodRepository, TenantId,
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
    last_event_created_at: Option<OffsetDateTime>,
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
            last_event_created_at: row.last_event_created_at,
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
                       is_default, last_event_created_at, created_at, deleted_at",
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
                    is_default, last_event_created_at, created_at, deleted_at \
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
                    is_default, last_event_created_at, created_at, deleted_at \
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

    async fn find_by_stripe_payment_method_id(
        &self,
        tenant_id: TenantId,
        stripe_payment_method_id: &str,
    ) -> Result<Option<PaymentMethod>, DomainError> {
        let row = sqlx::query_as::<_, PaymentMethodRow>(
            "SELECT id, tenant_id, customer_id, stripe_payment_method_id, brand, last4, \
                    is_default, last_event_created_at, created_at, deleted_at \
             FROM billing.payment_methods \
             WHERE tenant_id = $1 AND stripe_payment_method_id = $2 AND deleted_at IS NULL",
        )
        .bind(tenant_id.as_uuid())
        .bind(stripe_payment_method_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        Ok(row.map(Into::into))
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_event(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        stripe_payment_method_id: &str,
        brand: &str,
        last4: &str,
        is_default: bool,
        event_created_at: OffsetDateTime,
    ) -> Result<EventApplication, DomainError> {
        // Upsert with the ordering guard in the statement, the same shape as
        // the invoice one: fresh insert or admitted update is one row, a
        // conflict the predicate rejects is zero -- the stale signal.
        let result = sqlx::query(
            "INSERT INTO billing.payment_methods AS pm \
                 (id, tenant_id, customer_id, stripe_payment_method_id, brand, last4, \
                  is_default, last_event_created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (tenant_id, stripe_payment_method_id) WHERE deleted_at IS NULL \
             DO UPDATE SET \
                 customer_id = EXCLUDED.customer_id, \
                 brand = EXCLUDED.brand, \
                 last4 = EXCLUDED.last4, \
                 is_default = EXCLUDED.is_default, \
                 last_event_created_at = EXCLUDED.last_event_created_at \
             WHERE pm.last_event_created_at IS NULL \
                OR pm.last_event_created_at <= EXCLUDED.last_event_created_at",
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id.as_uuid())
        .bind(customer_id.as_uuid())
        .bind(stripe_payment_method_id)
        .bind(brand)
        .bind(last4)
        .bind(is_default)
        .bind(event_created_at)
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        if result.rows_affected() == 0 {
            Ok(EventApplication::Stale)
        } else {
            Ok(EventApplication::Applied)
        }
    }

    async fn detach_event(
        &self,
        tenant_id: TenantId,
        stripe_payment_method_id: &str,
        event_created_at: OffsetDateTime,
    ) -> Result<EventApplication, DomainError> {
        // Soft delete, ordering predicate in the WHERE clause. Precondition:
        // the row exists (the caller looked it up), so zero rows means a
        // newer event already landed -- Stale, not "no such row".
        let result = sqlx::query(
            "UPDATE billing.payment_methods \
                SET deleted_at = now(), last_event_created_at = $3 \
              WHERE tenant_id = $1 AND stripe_payment_method_id = $2 AND deleted_at IS NULL \
                AND (last_event_created_at IS NULL OR last_event_created_at <= $3)",
        )
        .bind(tenant_id.as_uuid())
        .bind(stripe_payment_method_id)
        .bind(event_created_at)
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        if result.rows_affected() == 0 {
            Ok(EventApplication::Stale)
        } else {
            Ok(EventApplication::Applied)
        }
    }

    async fn set_default(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        id: PaymentMethodId,
    ) -> Result<(), DomainError> {
        // One statement: is_default becomes true for exactly the target row
        // and false for every sibling, atomically -- no "two defaults or
        // none" window. No ordering predicate: this column, chosen this way,
        // has no webhook writer to race (see the port's rustdoc).
        sqlx::query(
            "UPDATE billing.payment_methods \
                SET is_default = (id = $3) \
              WHERE tenant_id = $1 AND customer_id = $2 AND deleted_at IS NULL",
        )
        .bind(tenant_id.as_uuid())
        .bind(customer_id.as_uuid())
        .bind(id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        Ok(())
    }
}
