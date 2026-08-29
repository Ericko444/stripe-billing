use domain::{
    DomainError, OutboundRequest, OutboundRequestId, OutboundRequestRepository, TenantId,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::RepositoryError;

#[derive(sqlx::FromRow)]
struct OutboundRequestRow {
    id: Uuid,
    tenant_id: Uuid,
    operation: String,
    request_fingerprint: String,
    idempotency_key: String,
    stripe_object_id: Option<String>,
    created_at: OffsetDateTime,
    completed_at: Option<OffsetDateTime>,
}

impl From<OutboundRequestRow> for OutboundRequest {
    fn from(row: OutboundRequestRow) -> Self {
        OutboundRequest {
            id: OutboundRequestId::new(row.id),
            tenant_id: TenantId::new(row.tenant_id),
            operation: row.operation,
            request_fingerprint: row.request_fingerprint,
            idempotency_key: row.idempotency_key,
            stripe_object_id: row.stripe_object_id,
            created_at: row.created_at,
            completed_at: row.completed_at,
        }
    }
}

fn to_domain_error(err: RepositoryError) -> DomainError {
    match err {
        RepositoryError::UniqueViolation => DomainError::Conflict,
        other => DomainError::Repository(other.to_string()),
    }
}

/// Postgres-backed `OutboundRequestRepository`.
pub struct PgOutboundRequestRepository {
    pool: PgPool,
}

impl PgOutboundRequestRepository {
    /// Wraps a `PgPool` as a `PgOutboundRequestRepository`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl OutboundRequestRepository for PgOutboundRequestRepository {
    async fn create(
        &self,
        tenant_id: TenantId,
        operation: String,
        request_fingerprint: String,
        idempotency_key: String,
    ) -> Result<OutboundRequest, DomainError> {
        let row = sqlx::query_as::<_, OutboundRequestRow>(
            "INSERT INTO billing.outbound_requests \
                 (id, tenant_id, operation, request_fingerprint, idempotency_key) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING id, tenant_id, operation, request_fingerprint, idempotency_key, \
                       stripe_object_id, created_at, completed_at",
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id.as_uuid())
        .bind(operation)
        .bind(request_fingerprint)
        .bind(idempotency_key)
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::classify)
        .map_err(to_domain_error)?;

        Ok(row.into())
    }

    async fn find_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<OutboundRequest>, DomainError> {
        let row = sqlx::query_as::<_, OutboundRequestRow>(
            "SELECT id, tenant_id, operation, request_fingerprint, idempotency_key, \
                    stripe_object_id, created_at, completed_at \
             FROM billing.outbound_requests \
             WHERE idempotency_key = $1",
        )
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        Ok(row.map(Into::into))
    }

    async fn find_by_fingerprint(
        &self,
        tenant_id: TenantId,
        operation: &str,
        request_fingerprint: &str,
    ) -> Result<Option<OutboundRequest>, DomainError> {
        let row = sqlx::query_as::<_, OutboundRequestRow>(
            "SELECT id, tenant_id, operation, request_fingerprint, idempotency_key, \
                    stripe_object_id, created_at, completed_at \
             FROM billing.outbound_requests \
             WHERE tenant_id = $1 AND operation = $2 AND request_fingerprint = $3 \
             ORDER BY created_at DESC \
             LIMIT 1",
        )
        .bind(tenant_id.as_uuid())
        .bind(operation)
        .bind(request_fingerprint)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        Ok(row.map(Into::into))
    }

    async fn mark_complete(
        &self,
        tenant_id: TenantId,
        id: OutboundRequestId,
        stripe_object_id: String,
    ) -> Result<OutboundRequest, DomainError> {
        let row = sqlx::query_as::<_, OutboundRequestRow>(
            "UPDATE billing.outbound_requests \
             SET completed_at = now(), stripe_object_id = $3 \
             WHERE tenant_id = $1 AND id = $2 \
             RETURNING id, tenant_id, operation, request_fingerprint, idempotency_key, \
                       stripe_object_id, created_at, completed_at",
        )
        .bind(tenant_id.as_uuid())
        .bind(id.as_uuid())
        .bind(stripe_object_id)
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        Ok(row.into())
    }
}
