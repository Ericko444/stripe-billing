use domain::{
    CustomerId, DomainError, EventApplication, Invoice, InvoiceCursor, InvoiceId, InvoicePage,
    InvoiceRepository, InvoiceStatus, Money, SubscriptionId, TenantId,
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
    last_event_created_at: Option<OffsetDateTime>,
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
            last_event_created_at: row.last_event_created_at,
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
                       amount_minor, currency, status, last_event_created_at, created_at, deleted_at",
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
                    amount_minor, currency, status, last_event_created_at, created_at, deleted_at \
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
                    amount_minor, currency, status, last_event_created_at, created_at, deleted_at \
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

    async fn list_page(
        &self,
        tenant_id: TenantId,
        after: Option<InvoiceCursor>,
        limit: u16,
    ) -> Result<InvoicePage, DomainError> {
        // The keyset seek is one row-value comparison, not
        // `created_at < $2 OR (created_at = $2 AND id < $3)` -- Postgres
        // compares tuples lexicographically and can walk
        // `invoices_tenant_created_id_idx` (0013) directly. When `after` is
        // NULL the `$2::timestamptz IS NULL` guard short-circuits the seek and
        // the scan starts at the newest row.
        //
        // Fetch one row past `limit`: its presence is the only reliable
        // "there is a next page" signal, so `next` is never a cursor that
        // would yield nothing.
        let (after_ts, after_id) = match after {
            Some(cursor) => (Some(cursor.created_at()), Some(cursor.id().as_uuid())),
            None => (None, None),
        };
        let fetch_limit = i64::from(limit) + 1;

        let mut rows = sqlx::query_as::<_, InvoiceRow>(
            "SELECT id, tenant_id, customer_id, subscription_id, stripe_invoice_id, \
                    amount_minor, currency, status, last_event_created_at, created_at, deleted_at \
             FROM billing.invoices \
             WHERE tenant_id = $1 AND deleted_at IS NULL \
               AND ($2::timestamptz IS NULL \
                    OR (created_at, id) < ($2::timestamptz, $3::uuid)) \
             ORDER BY created_at DESC, id DESC \
             LIMIT $4",
        )
        .bind(tenant_id.as_uuid())
        .bind(after_ts)
        .bind(after_id)
        .bind(fetch_limit)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        let has_more = rows.len() > usize::from(limit);
        rows.truncate(usize::from(limit));
        let items: Vec<Invoice> = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?;

        let next = if has_more {
            items
                .last()
                .map(|last| InvoiceCursor::new(last.created_at, last.id))
        } else {
            None
        };

        Ok(InvoicePage { items, next })
    }

    async fn find_by_stripe_invoice_id(
        &self,
        tenant_id: TenantId,
        stripe_invoice_id: &str,
    ) -> Result<Option<Invoice>, DomainError> {
        let row = sqlx::query_as::<_, InvoiceRow>(
            "SELECT id, tenant_id, customer_id, subscription_id, stripe_invoice_id, \
                    amount_minor, currency, status, last_event_created_at, created_at, deleted_at \
             FROM billing.invoices \
             WHERE tenant_id = $1 AND stripe_invoice_id = $2 AND deleted_at IS NULL",
        )
        .bind(tenant_id.as_uuid())
        .bind(stripe_invoice_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        row.map(TryInto::try_into).transpose()
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_event(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        subscription_id: Option<SubscriptionId>,
        stripe_invoice_id: &str,
        amount: Money,
        status: InvoiceStatus,
        event_created_at: OffsetDateTime,
    ) -> Result<EventApplication, DomainError> {
        // One statement: insert the mirror if it is new, otherwise update it
        // only when this event is not older than the last one applied. The
        // `AS inv` alias lets the DO UPDATE predicate name the existing row;
        // `rows_affected() == 0` means the conflict fired and the predicate
        // rejected it -- the stale signal, resolved by Postgres.
        let result = sqlx::query(
            "INSERT INTO billing.invoices AS inv \
                 (id, tenant_id, customer_id, subscription_id, stripe_invoice_id, \
                  amount_minor, currency, status, last_event_created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (tenant_id, stripe_invoice_id) WHERE deleted_at IS NULL \
             DO UPDATE SET \
                 customer_id = EXCLUDED.customer_id, \
                 subscription_id = EXCLUDED.subscription_id, \
                 amount_minor = EXCLUDED.amount_minor, \
                 currency = EXCLUDED.currency, \
                 status = EXCLUDED.status, \
                 last_event_created_at = EXCLUDED.last_event_created_at \
             WHERE inv.last_event_created_at IS NULL \
                OR inv.last_event_created_at <= EXCLUDED.last_event_created_at",
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id.as_uuid())
        .bind(customer_id.as_uuid())
        .bind(subscription_id.map(|id| id.as_uuid()))
        .bind(stripe_invoice_id)
        .bind(amount.amount_minor())
        .bind(currency_code(amount.currency()))
        .bind(status.as_str())
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
}
