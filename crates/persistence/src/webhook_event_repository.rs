use domain::{DomainError, TenantId, WebhookEvent, WebhookEventId, WebhookEventRepository};
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::RepositoryError;

#[derive(sqlx::FromRow)]
struct WebhookEventRow {
    id: Uuid,
    tenant_id: Option<Uuid>,
    stripe_event_id: String,
    event_type: String,
    payload: Value,
    created_at: OffsetDateTime,
    processed_at: Option<OffsetDateTime>,
}

impl From<WebhookEventRow> for WebhookEvent {
    fn from(row: WebhookEventRow) -> Self {
        WebhookEvent {
            id: WebhookEventId::new(row.id),
            tenant_id: row.tenant_id.map(TenantId::new),
            stripe_event_id: row.stripe_event_id,
            event_type: row.event_type,
            payload: row.payload,
            created_at: row.created_at,
            processed_at: row.processed_at,
        }
    }
}

fn to_domain_error(err: RepositoryError) -> DomainError {
    DomainError::Repository(err.to_string())
}

/// Postgres-backed `WebhookEventRepository`.
pub struct PgWebhookEventRepository {
    pool: PgPool,
}

impl PgWebhookEventRepository {
    /// Wraps a `PgPool` as a `PgWebhookEventRepository`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl WebhookEventRepository for PgWebhookEventRepository {
    async fn create(
        &self,
        tenant_id: Option<TenantId>,
        stripe_event_id: String,
        event_type: String,
        payload: Value,
    ) -> Result<WebhookEvent, DomainError> {
        let row = sqlx::query_as::<_, WebhookEventRow>(
            "INSERT INTO billing.webhook_events \
                 (id, tenant_id, stripe_event_id, event_type, payload) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING id, tenant_id, stripe_event_id, event_type, payload, created_at, \
                       processed_at",
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id.map(|t| t.as_uuid()))
        .bind(stripe_event_id)
        .bind(event_type)
        .bind(payload)
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        Ok(row.into())
    }

    async fn find_by_stripe_event_id(
        &self,
        stripe_event_id: &str,
    ) -> Result<Option<WebhookEvent>, DomainError> {
        let row = sqlx::query_as::<_, WebhookEventRow>(
            "SELECT id, tenant_id, stripe_event_id, event_type, payload, created_at, processed_at \
             FROM billing.webhook_events \
             WHERE stripe_event_id = $1",
        )
        .bind(stripe_event_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        Ok(row.map(Into::into))
    }
}
