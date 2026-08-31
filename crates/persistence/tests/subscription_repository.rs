mod common;

use std::error::Error;

use domain::{
    Currency, CustomerId, CustomerRepository, DomainError, EventApplication, Money, PlanId,
    PlanRepository, Subscription, SubscriptionRepository, SubscriptionStatus, TenantId,
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
    assert_eq!(created.last_event_created_at, None);
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

#[tokio::test]
async fn find_by_stripe_subscription_id_is_scoped_to_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant_a = TenantId::new(Uuid::new_v4());
    let tenant_b = TenantId::new(Uuid::new_v4());
    let (customer_a, plan_a) = seed(&db.pool, tenant_a).await?;

    let created =
        create_subscription(&repo, tenant_a, customer_a, plan_a, "sub_find_by_stripe_id").await?;

    let found_own_tenant = repo
        .find_by_stripe_subscription_id(tenant_a, "sub_find_by_stripe_id")
        .await?;
    let found_other_tenant = repo
        .find_by_stripe_subscription_id(tenant_b, "sub_find_by_stripe_id")
        .await?;
    let found_unknown_id = repo
        .find_by_stripe_subscription_id(tenant_a, "sub_never_created")
        .await?;

    assert_eq!(found_own_tenant, Some(created));
    assert_eq!(found_other_tenant, None);
    assert_eq!(found_unknown_id, None);
    Ok(())
}

#[tokio::test]
async fn apply_event_updates_the_row_and_advances_the_ordering_column() -> Result<(), Box<dyn Error>>
{
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created =
        create_subscription(&repo, tenant, customer_id, plan_id, "sub_apply_event").await?;

    let event_created_at = OffsetDateTime::now_utc();
    let new_period_start = event_created_at;
    let new_period_end = event_created_at + Duration::days(30);

    let outcome = repo
        .apply_event(
            tenant,
            created.id,
            SubscriptionStatus::PastDue,
            new_period_start,
            new_period_end,
            true,
            event_created_at,
        )
        .await?;

    assert_eq!(outcome, EventApplication::Applied);

    let found = repo.find(tenant, created.id).await?.ok_or("row exists")?;
    assert_eq!(found.status, SubscriptionStatus::PastDue);
    assert_eq!(found.current_period_start, new_period_start);
    assert_eq!(found.current_period_end, new_period_end);
    assert!(found.cancel_at_period_end);
    assert_eq!(found.last_event_created_at, Some(event_created_at));
    // apply_event never touches plan_id -- it isn't a parameter this method
    // accepts, and the row should still point at the plan it was created with.
    assert_eq!(found.plan_id, created.plan_id);
    Ok(())
}

#[tokio::test]
async fn apply_event_with_an_older_timestamp_is_stale_and_leaves_the_row_unchanged()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created = create_subscription(&repo, tenant, customer_id, plan_id, "sub_stale").await?;

    let newer_created_at = OffsetDateTime::now_utc();
    let newer_period_start = newer_created_at;
    let newer_period_end = newer_created_at + Duration::days(30);
    let first_outcome = repo
        .apply_event(
            tenant,
            created.id,
            SubscriptionStatus::Active,
            newer_period_start,
            newer_period_end,
            false,
            newer_created_at,
        )
        .await?;
    assert_eq!(first_outcome, EventApplication::Applied);

    // An older, out-of-order redelivery: an earlier `created`, a different
    // status, different periods -- everything the guard should refuse to
    // let overwrite the row just written above.
    let older_created_at = newer_created_at - Duration::minutes(5);
    let stale_outcome = repo
        .apply_event(
            tenant,
            created.id,
            SubscriptionStatus::Canceled,
            older_created_at,
            older_created_at + Duration::days(1),
            true,
            older_created_at,
        )
        .await?;

    assert_eq!(stale_outcome, EventApplication::Stale);

    let found = repo.find(tenant, created.id).await?.ok_or("row exists")?;
    assert_eq!(found.status, SubscriptionStatus::Active);
    assert_eq!(found.current_period_start, newer_period_start);
    assert_eq!(found.current_period_end, newer_period_end);
    assert!(!found.cancel_at_period_end);
    assert_eq!(found.last_event_created_at, Some(newer_created_at));
    Ok(())
}

#[tokio::test]
async fn apply_event_with_an_equal_timestamp_applies() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created = create_subscription(&repo, tenant, customer_id, plan_id, "sub_equal").await?;

    // Stripe's `created` has second granularity, so two genuine events can
    // legitimately share one -- equality must apply, not be treated as
    // stale, or the second of the two would be silently dropped.
    let shared_created_at = OffsetDateTime::now_utc();
    let first_outcome = repo
        .apply_event(
            tenant,
            created.id,
            SubscriptionStatus::Active,
            shared_created_at,
            shared_created_at + Duration::days(30),
            false,
            shared_created_at,
        )
        .await?;
    assert_eq!(first_outcome, EventApplication::Applied);

    let second_outcome = repo
        .apply_event(
            tenant,
            created.id,
            SubscriptionStatus::PastDue,
            shared_created_at,
            shared_created_at + Duration::days(30),
            true,
            shared_created_at,
        )
        .await?;

    assert_eq!(second_outcome, EventApplication::Applied);

    let found = repo.find(tenant, created.id).await?.ok_or("row exists")?;
    assert_eq!(found.status, SubscriptionStatus::PastDue);
    assert!(found.cancel_at_period_end);
    Ok(())
}
