mod common;

use std::error::Error;

use domain::{Currency, Money, PlanRepository, TenantId};
use persistence::PgPlanRepository;
use uuid::Uuid;

#[tokio::test]
async fn create_then_find() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgPlanRepository::new(db.pool.clone());
    let tenant_id = TenantId::new(Uuid::new_v4());

    let created = repo
        .create(
            tenant_id,
            "price_1".to_string(),
            "prod_1".to_string(),
            "Pro monthly".to_string(),
            Money::new(1500, Currency::Eur),
        )
        .await?;
    let found = repo.find(tenant_id, created.id).await?;

    assert_eq!(found, Some(created.clone()));
    // Money survives the (amount_minor, currency CHAR(3)) round trip.
    assert_eq!(created.amount, Money::new(1500, Currency::Eur));
    Ok(())
}

#[tokio::test]
async fn list_is_scoped_to_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgPlanRepository::new(db.pool.clone());
    let tenant_a = TenantId::new(Uuid::new_v4());
    let tenant_b = TenantId::new(Uuid::new_v4());

    let plan_a = repo
        .create(
            tenant_a,
            "price_a".to_string(),
            "prod_a".to_string(),
            "Plan A".to_string(),
            Money::new(1000, Currency::Usd),
        )
        .await?;
    repo.create(
        tenant_b,
        "price_b".to_string(),
        "prod_b".to_string(),
        "Plan B".to_string(),
        Money::new(2000, Currency::Usd),
    )
    .await?;

    let list_a = repo.list(tenant_a).await?;

    assert_eq!(list_a, vec![plan_a]);
    Ok(())
}

#[tokio::test]
async fn soft_deleted_plan_is_excluded_from_reads() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgPlanRepository::new(db.pool.clone());
    let tenant_id = TenantId::new(Uuid::new_v4());

    let created = repo
        .create(
            tenant_id,
            "price_soft_delete".to_string(),
            "prod_soft_delete".to_string(),
            "Soon gone".to_string(),
            Money::new(500, Currency::Gbp),
        )
        .await?;
    sqlx::query("UPDATE billing.plans SET deleted_at = now() WHERE id = $1")
        .bind(created.id.as_uuid())
        .execute(&db.pool)
        .await?;

    let found = repo.find(tenant_id, created.id).await?;
    let list = repo.list(tenant_id).await?;

    assert_eq!(found, None);
    assert_eq!(list, Vec::new());
    Ok(())
}

#[tokio::test]
async fn stripe_price_id_can_be_reused_after_soft_delete() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgPlanRepository::new(db.pool.clone());
    let tenant_id = TenantId::new(Uuid::new_v4());

    let first = repo
        .create(
            tenant_id,
            "price_reuse".to_string(),
            "prod_reuse".to_string(),
            "First".to_string(),
            Money::new(1000, Currency::Usd),
        )
        .await?;
    sqlx::query("UPDATE billing.plans SET deleted_at = now() WHERE id = $1")
        .bind(first.id.as_uuid())
        .execute(&db.pool)
        .await?;

    let second = repo
        .create(
            tenant_id,
            "price_reuse".to_string(),
            "prod_reuse".to_string(),
            "Second".to_string(),
            Money::new(1000, Currency::Usd),
        )
        .await?;
    let list = repo.list(tenant_id).await?;

    assert_eq!(list, vec![second]);
    Ok(())
}
