use domain::{
    CustomerId, DomainError, Invoice, InvoiceId, InvoiceRepository, InvoiceStatus, Money,
    SubscriptionId, TenantId,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::RepositoryError;
use crate::currency_codec::{currency_code, currency_from_code};

#[derive(sqlx::FromRow)]
struct InvoiceRow {
    id: Uuid,
    tenant_id: Uuid,
    customer_id: Uuid,
    subscription_id: Option<Uuid>,
    stripe_invoice_id: String,
    amount_minor: i64,
    currency: String,
    status: String,
    created_at: OffsetDateTime,
    deleted_at: Option<OffsetDateTime>,
}

impl TryFrom<InvoiceRow> for Invoice {
    type Error = DomainError;

    fn try_from(row: InvoiceRow) -> Result<Self, Self::Error> {
        Ok(Invoice {
            id: InvoiceId::new(row.id),
            tenant_id: TenantId::new(row.tenant_id),
            customer_id: CustomerId::new(row.customer_id),
            subscription_id: row.subscription_id.map(SubscriptionId::new),
            stripe_invoice_id: row.stripe_invoice_id,
            amount: Money::new(row.amount_minor, currency_from_code(row.currency.trim())?),
            status: InvoiceStatus::try_from(row.status.as_str())?,
            created_at: row.created_at,
            deleted_at: row.deleted_at,
        })
    }
}

fn to_domain_error(err: RepositoryError) -> DomainError {
    DomainError::Repository(err.to_string())
}

/// Postgres-backed `InvoiceRepository`.
pub struct PgInvoiceRepository {
    pool: PgPool,
}

impl PgInvoiceRepository {
    /// Wraps a `PgPool` as a `PgInvoiceRepository`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl InvoiceRepository for PgInvoiceRepository {
    async fn create(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        subscription_id: Option<SubscriptionId>,
        stripe_invoice_id: String,
        amount: Money,
        status: InvoiceStatus,
    ) -> Result<Invoice, DomainError> {
        let row = sqlx::query_as::<_, InvoiceRow>(
            "INSERT INTO billing.invoices \
                 (id, tenant_id, customer_id, subscription_id, stripe_invoice_id, \
                  amount_minor, currency, status) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             RETURNING id, tenant_id, customer_id, subscription_id, stripe_invoice_id, \
                       amount_minor, currency, status, created_at, deleted_at",
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id.as_uuid())
        .bind(customer_id.as_uuid())
        .bind(subscription_id.map(|id| id.as_uuid()))
        .bind(stripe_invoice_id)
        .bind(amount.amount_minor())
        .bind(currency_code(amount.currency()))
        .bind(status.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        row.try_into()
    }

    async fn find(
        &self,
        tenant_id: TenantId,
        id: InvoiceId,
    ) -> Result<Option<Invoice>, DomainError> {
        let row = sqlx::query_as::<_, InvoiceRow>(
            "SELECT id, tenant_id, customer_id, subscription_id, stripe_invoice_id, \
                    amount_minor, currency, status, created_at, deleted_at \
             FROM billing.invoices \
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

    async fn list(&self, tenant_id: TenantId) -> Result<Vec<Invoice>, DomainError> {
        let rows = sqlx::query_as::<_, InvoiceRow>(
            "SELECT id, tenant_id, customer_id, subscription_id, stripe_invoice_id, \
                    amount_minor, currency, status, created_at, deleted_at \
             FROM billing.invoices \
             WHERE tenant_id = $1 AND deleted_at IS NULL",
        )
        .bind(tenant_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        rows.into_iter().map(TryInto::try_into).collect()
    }
}
