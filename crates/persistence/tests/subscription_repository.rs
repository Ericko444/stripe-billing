mod common;

use std::error::Error;

use domain::{
    Currency, CustomerId, CustomerRepository, DomainError, Money, PlanId, PlanRepository,
    Subscription, SubscriptionRepository, SubscriptionStatus, TenantId,
};
use persistence::{PgCustomerRepository, PgPlanRepository, PgSubscriptionRepository};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Inserts a customer and a plan for `tenant` and returns their ids, so a
/// subscription's FKs point at real rows.
async fn seed(pool: &PgPool, tenant: TenantId) -> Result<(CustomerId, PlanId), Box<dyn Error>> {
    let customer = PgCustomerRepository::new(pool.clone())
        .create(tenant, Some(format!("cus_{}", Uuid::new_v4())))
        .await?;
    let plan = PgPlanRepository::new(pool.clone())
        .create(
            tenant,
            format!("price_{}", Uuid::new_v4()),
            format!("prod_{}", Uuid::new_v4()),
            "Seed plan".to_string(),
            Money::new(1000, Currency::Usd),
        )
        .await?;
    Ok((customer.id, plan.id))
}

async fn create_subscription(
    repo: &PgSubscriptionRepository,
    tenant: TenantId,
    customer_id: CustomerId,
    plan_id: PlanId,
    stripe_subscription_id: &str,
) -> Result<Subscription, DomainError> {
    let now = OffsetDateTime::now_utc();
    repo.create(
        tenant,
        customer_id,
        plan_id,
        stripe_subscription_id.to_string(),
        format!("si_{stripe_subscription_id}"),
        SubscriptionStatus::Active,
        now,
        now + Duration::days(30),
    )
    .await
}

#[tokio::test]
async fn create_then_find() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;

    let created = create_subscription(&repo, tenant, customer_id, plan_id, "sub_1").await?;
    let found = repo.find(tenant, created.id).await?;

    assert_eq!(found, Some(created.clone()));
    assert_eq!(created.status, SubscriptionStatus::Active);
    assert!(!created.cancel_at_period_end);
    Ok(())
}

#[tokio::test]
async fn list_is_scoped_to_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant_a = TenantId::new(Uuid::new_v4());
    let tenant_b = TenantId::new(Uuid::new_v4());
    let (customer_a, plan_a) = seed(&db.pool, tenant_a).await?;
    let (customer_b, plan_b) = seed(&db.pool, tenant_b).await?;

    let sub_a = create_subscription(&repo, tenant_a, customer_a, plan_a, "sub_a").await?;
    create_subscription(&repo, tenant_b, customer_b, plan_b, "sub_b").await?;

    let list_a = repo.list(tenant_a).await?;

    assert_eq!(list_a, vec![sub_a]);
    Ok(())
}

#[tokio::test]
async fn soft_deleted_subscription_is_excluded_from_reads() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;

    let created =
        create_subscription(&repo, tenant, customer_id, plan_id, "sub_soft_delete").await?;
    sqlx::query("UPDATE billing.subscriptions SET deleted_at = now() WHERE id = $1")
        .bind(created.id.as_uuid())
        .execute(&db.pool)
        .await?;

    let found = repo.find(tenant, created.id).await?;
    let list = repo.list(tenant).await?;

    assert_eq!(found, None);
    assert_eq!(list, Vec::new());
    Ok(())
}

#[tokio::test]
async fn stripe_subscription_id_can_be_reused_after_soft_delete() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;

    let first = create_subscription(&repo, tenant, customer_id, plan_id, "sub_reuse").await?;
    sqlx::query("UPDATE billing.subscriptions SET deleted_at = now() WHERE id = $1")
        .bind(first.id.as_uuid())
        .execute(&db.pool)
        .await?;

    let second = create_subscription(&repo, tenant, customer_id, plan_id, "sub_reuse").await?;
    let list = repo.list(tenant).await?;

    assert_eq!(list, vec![second]);
    Ok(())
}

#[tokio::test]
async fn create_with_unknown_customer_fails_on_fk() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (_customer_id, plan_id) = seed(&db.pool, tenant).await?;

    let result = create_subscription(
        &repo,
        tenant,
        CustomerId::new(Uuid::new_v4()),
        plan_id,
        "sub_fk",
    )
    .await;

    assert!(matches!(result, Err(DomainError::Repository(_))));
    Ok(())
}
