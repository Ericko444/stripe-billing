use domain::{DomainError, Money, Plan, PlanId, PlanRepository, TenantId};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::RepositoryError;
use crate::currency_codec::{currency_code, currency_from_code};

#[derive(sqlx::FromRow)]
struct PlanRow {
    id: Uuid,
    tenant_id: Uuid,
    stripe_price_id: String,
    stripe_product_id: String,
    name: String,
    amount_minor: i64,
    currency: String,
    created_at: OffsetDateTime,
    deleted_at: Option<OffsetDateTime>,
}

impl TryFrom<PlanRow> for Plan {
    type Error = DomainError;

    fn try_from(row: PlanRow) -> Result<Self, Self::Error> {
        Ok(Plan {
            id: PlanId::new(row.id),
            tenant_id: TenantId::new(row.tenant_id),
            stripe_price_id: row.stripe_price_id,
            stripe_product_id: row.stripe_product_id,
            name: row.name,
            amount: Money::new(row.amount_minor, currency_from_code(row.currency.trim())?),
            created_at: row.created_at,
            deleted_at: row.deleted_at,
        })
    }
}

fn to_domain_error(err: RepositoryError) -> DomainError {
    DomainError::Repository(err.to_string())
}

/// Postgres-backed `PlanRepository`.
pub struct PgPlanRepository {
    pool: PgPool,
}

impl PgPlanRepository {
    /// Wraps a `PgPool` as a `PgPlanRepository`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl PlanRepository for PgPlanRepository {
    async fn create(
        &self,
        tenant_id: TenantId,
        stripe_price_id: String,
        stripe_product_id: String,
        name: String,
        amount: Money,
    ) -> Result<Plan, DomainError> {
        let row = sqlx::query_as::<_, PlanRow>(
            "INSERT INTO billing.plans \
                 (id, tenant_id, stripe_price_id, stripe_product_id, name, amount_minor, currency) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             RETURNING id, tenant_id, stripe_price_id, stripe_product_id, name, \
                       amount_minor, currency, created_at, deleted_at",
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id.as_uuid())
        .bind(stripe_price_id)
        .bind(stripe_product_id)
        .bind(name)
        .bind(amount.amount_minor())
        .bind(currency_code(amount.currency()))
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        row.try_into()
    }

    async fn find(&self, tenant_id: TenantId, id: PlanId) -> Result<Option<Plan>, DomainError> {
        let row = sqlx::query_as::<_, PlanRow>(
            "SELECT id, tenant_id, stripe_price_id, stripe_product_id, name, \
                    amount_minor, currency, created_at, deleted_at \
             FROM billing.plans \
             WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(tenant_id.as_uuid())
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        row.map(TryInto::try_into).transpose()
    }

    async fn list(&self, tenant_id: TenantId) -> Result<Vec<Plan>, DomainError> {
        let rows = sqlx::query_as::<_, PlanRow>(
            "SELECT id, tenant_id, stripe_price_id, stripe_product_id, name, \
                    amount_minor, currency, created_at, deleted_at \
             FROM billing.plans \
             WHERE tenant_id = $1 AND deleted_at IS NULL",
        )
        .bind(tenant_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        rows.into_iter().map(TryInto::try_into).collect()
    }

    async fn find_by_stripe_price_id(
        &self,
        tenant_id: TenantId,
        stripe_price_id: &str,
    ) -> Result<Option<Plan>, DomainError> {
        let row = sqlx::query_as::<_, PlanRow>(
            "SELECT id, tenant_id, stripe_price_id, stripe_product_id, name, \
                    amount_minor, currency, created_at, deleted_at \
             FROM billing.plans \
             WHERE tenant_id = $1 AND stripe_price_id = $2 AND deleted_at IS NULL",
        )
        .bind(tenant_id.as_uuid())
        .bind(stripe_price_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        row.map(TryInto::try_into).transpose()
    }
}
