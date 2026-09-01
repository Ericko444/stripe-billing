use domain::{
    CustomerId, DomainError, EventApplication, PlanId, Subscription, SubscriptionId,
    SubscriptionRepository, SubscriptionStatus, TenantId,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::RepositoryError;

#[derive(sqlx::FromRow)]
struct SubscriptionRow {
    id: Uuid,
    tenant_id: Uuid,
    customer_id: Uuid,
    plan_id: Uuid,
    stripe_subscription_id: String,
    stripe_subscription_item_id: String,
    status: String,
    current_period_start: OffsetDateTime,
    current_period_end: OffsetDateTime,
    cancel_at_period_end: bool,
    last_event_created_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    deleted_at: Option<OffsetDateTime>,
}

impl TryFrom<SubscriptionRow> for Subscription {
    type Error = DomainError;

    fn try_from(row: SubscriptionRow) -> Result<Self, Self::Error> {
        Ok(Subscription {
            id: SubscriptionId::new(row.id),
            tenant_id: TenantId::new(row.tenant_id),
            customer_id: CustomerId::new(row.customer_id),
            plan_id: PlanId::new(row.plan_id),
            stripe_subscription_id: row.stripe_subscription_id,
            stripe_subscription_item_id: row.stripe_subscription_item_id,
            status: SubscriptionStatus::try_from(row.status.as_str())?,
            current_period_start: row.current_period_start,
            current_period_end: row.current_period_end,
            cancel_at_period_end: row.cancel_at_period_end,
            last_event_created_at: row.last_event_created_at,
            created_at: row.created_at,
            deleted_at: row.deleted_at,
        })
    }
}

fn to_domain_error(err: RepositoryError) -> DomainError {
    DomainError::Repository(err.to_string())
}

/// Postgres-backed `SubscriptionRepository`.
pub struct PgSubscriptionRepository {
    pool: PgPool,
}

impl PgSubscriptionRepository {
    /// Wraps a `PgPool` as a `PgSubscriptionRepository`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl SubscriptionRepository for PgSubscriptionRepository {
    #[allow(clippy::too_many_arguments)]
    async fn create(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        plan_id: PlanId,
        stripe_subscription_id: String,
        stripe_subscription_item_id: String,
        status: SubscriptionStatus,
        current_period_start: OffsetDateTime,
        current_period_end: OffsetDateTime,
    ) -> Result<Subscription, DomainError> {
        let row = sqlx::query_as::<_, SubscriptionRow>(
            "INSERT INTO billing.subscriptions \
                 (id, tenant_id, customer_id, plan_id, stripe_subscription_id, \
                  stripe_subscription_item_id, status, current_period_start, current_period_end) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             RETURNING id, tenant_id, customer_id, plan_id, stripe_subscription_id, \
                       stripe_subscription_item_id, status, current_period_start, \
                       current_period_end, cancel_at_period_end, last_event_created_at, created_at, deleted_at",
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id.as_uuid())
        .bind(customer_id.as_uuid())
        .bind(plan_id.as_uuid())
        .bind(stripe_subscription_id)
        .bind(stripe_subscription_item_id)
        .bind(status.as_str())
        .bind(current_period_start)
        .bind(current_period_end)
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        row.try_into()
    }

    async fn find(
        &self,
        tenant_id: TenantId,
        id: SubscriptionId,
    ) -> Result<Option<Subscription>, DomainError> {
        let row = sqlx::query_as::<_, SubscriptionRow>(
            "SELECT id, tenant_id, customer_id, plan_id, stripe_subscription_id, \
                    stripe_subscription_item_id, status, current_period_start, \
                    current_period_end, cancel_at_period_end, last_event_created_at, \
                    created_at, deleted_at \
             FROM billing.subscriptions \
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

    async fn list(&self, tenant_id: TenantId) -> Result<Vec<Subscription>, DomainError> {
        let rows = sqlx::query_as::<_, SubscriptionRow>(
            "SELECT id, tenant_id, customer_id, plan_id, stripe_subscription_id, \
                    stripe_subscription_item_id, status, current_period_start, \
                    current_period_end, cancel_at_period_end, last_event_created_at, \
                    created_at, deleted_at \
             FROM billing.subscriptions \
             WHERE tenant_id = $1 AND deleted_at IS NULL",
        )
        .bind(tenant_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        rows.into_iter().map(TryInto::try_into).collect()
    }

    async fn find_by_stripe_subscription_id(
        &self,
        tenant_id: TenantId,
        stripe_subscription_id: &str,
    ) -> Result<Option<Subscription>, DomainError> {
        let row = sqlx::query_as::<_, SubscriptionRow>(
            "SELECT id, tenant_id, customer_id, plan_id, stripe_subscription_id, \
                    stripe_subscription_item_id, status, current_period_start, \
                    current_period_end, cancel_at_period_end, last_event_created_at, \
                    created_at, deleted_at \
             FROM billing.subscriptions \
             WHERE tenant_id = $1 AND stripe_subscription_id = $2 AND deleted_at IS NULL",
        )
        .bind(tenant_id.as_uuid())
        .bind(stripe_subscription_id)
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
        id: SubscriptionId,
        status: SubscriptionStatus,
        current_period_start: OffsetDateTime,
        current_period_end: OffsetDateTime,
        cancel_at_period_end: bool,
        event_created_at: OffsetDateTime,
    ) -> Result<EventApplication, DomainError> {
        // The ordering predicate is part of the statement, not a read-then-
        // compare in Rust -- see the port's rustdoc for why. rows_affected()
        // is the stale signal, reported by Postgres rather than concluded by
        // us.
        let result = sqlx::query(
            "UPDATE billing.subscriptions \
                SET status = $3, current_period_start = $4, current_period_end = $5, \
                    cancel_at_period_end = $6, last_event_created_at = $7 \
              WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL \
                AND (last_event_created_at IS NULL OR last_event_created_at <= $7)",
        )
        .bind(tenant_id.as_uuid())
        .bind(id.as_uuid())
        .bind(status.as_str())
        .bind(current_period_start)
        .bind(current_period_end)
        .bind(cancel_at_period_end)
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

    async fn set_plan(
        &self,
        tenant_id: TenantId,
        id: SubscriptionId,
        plan_id: PlanId,
    ) -> Result<(), DomainError> {
        // No ordering predicate, unlike `apply_event`: no webhook ever writes
        // this column, so there is no event to be stale against -- see the
        // port's rustdoc. Tenant-scoped like every other statement here, so a
        // wrong tenant updates nothing rather than another tenant's row.
        sqlx::query(
            "UPDATE billing.subscriptions \
                SET plan_id = $3 \
              WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(tenant_id.as_uuid())
        .bind(id.as_uuid())
        .bind(plan_id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::from)
        .map_err(to_domain_error)?;

        Ok(())
    }
}
