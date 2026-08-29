mod common;

use std::error::Error;

use domain::{
    Currency, CustomerId, CustomerRepository, DomainError, Invoice, InvoiceRepository,
    InvoiceStatus, Money, PlanRepository, SubscriptionId, SubscriptionRepository,
    SubscriptionStatus, TenantId,
};
use persistence::{
    PgCustomerRepository, PgInvoiceRepository, PgPlanRepository, PgSubscriptionRepository,
};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn seed_customer(pool: &PgPool, tenant: TenantId) -> Result<CustomerId, Box<dyn Error>> {
    let customer = PgCustomerRepository::new(pool.clone())
        .create(tenant, Some(format!("cus_{}", Uuid::new_v4())))
        .await?;
    Ok(customer.id)
}

/// Creates a plan + subscription for `tenant`/`customer_id` and returns the
/// subscription id, so an invoice can link to a real subscription row.
async fn seed_subscription(
    pool: &PgPool,
    tenant: TenantId,
    customer_id: CustomerId,
) -> Result<SubscriptionId, Box<dyn Error>> {
    let plan = PgPlanRepository::new(pool.clone())
        .create(
            tenant,
            format!("price_{}", Uuid::new_v4()),
            format!("prod_{}", Uuid::new_v4()),
            "Seed plan".to_string(),
            Money::new(1000, Currency::Usd),
        )
        .await?;
    let now = OffsetDateTime::now_utc();
    let subscription = PgSubscriptionRepository::new(pool.clone())
        .create(
            tenant,
            customer_id,
            plan.id,
            format!("sub_{}", Uuid::new_v4()),
            format!("si_{}", Uuid::new_v4()),
            SubscriptionStatus::Active,
            now,
            now + Duration::days(30),
        )
        .await?;
    Ok(subscription.id)
}

async fn create_invoice(
    repo: &PgInvoiceRepository,
    tenant: TenantId,
    customer_id: CustomerId,
    subscription_id: Option<SubscriptionId>,
    stripe_invoice_id: &str,
) -> Result<Invoice, DomainError> {
    repo.create(
        tenant,
        customer_id,
        subscription_id,
        stripe_invoice_id.to_string(),
        Money::new(4200, Currency::Eur),
        InvoiceStatus::Open,
    )
    .await
}

#[tokio::test]
async fn create_then_find() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;
    let subscription_id = seed_subscription(&db.pool, tenant, customer_id).await?;

    let created = create_invoice(&repo, tenant, customer_id, Some(subscription_id), "in_1").await?;
    let found = repo.find(tenant, created.id).await?;

    assert_eq!(found, Some(created.clone()));
    assert_eq!(created.amount, Money::new(4200, Currency::Eur));
    assert_eq!(created.subscription_id, Some(subscription_id));
    assert_eq!(created.status, InvoiceStatus::Open);
    Ok(())
}

#[tokio::test]
async fn invoice_without_subscription_is_allowed() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let created = create_invoice(&repo, tenant, customer_id, None, "in_no_sub").await?;
    let found = repo.find(tenant, created.id).await?;

    assert_eq!(found, Some(created.clone()));
    assert_eq!(created.subscription_id, None);
    Ok(())
}

#[tokio::test]
async fn list_is_scoped_to_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant_a = TenantId::new(Uuid::new_v4());
    let tenant_b = TenantId::new(Uuid::new_v4());
    let customer_a = seed_customer(&db.pool, tenant_a).await?;
    let customer_b = seed_customer(&db.pool, tenant_b).await?;

    let invoice_a = create_invoice(&repo, tenant_a, customer_a, None, "in_a").await?;
    create_invoice(&repo, tenant_b, customer_b, None, "in_b").await?;

    let list_a = repo.list(tenant_a).await?;

    assert_eq!(list_a, vec![invoice_a]);
    Ok(())
}

#[tokio::test]
async fn soft_deleted_invoice_is_excluded_from_reads() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let created = create_invoice(&repo, tenant, customer_id, None, "in_soft_delete").await?;
    sqlx::query("UPDATE billing.invoices SET deleted_at = now() WHERE id = $1")
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
async fn stripe_invoice_id_can_be_reused_after_soft_delete() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let first = create_invoice(&repo, tenant, customer_id, None, "in_reuse").await?;
    sqlx::query("UPDATE billing.invoices SET deleted_at = now() WHERE id = $1")
        .bind(first.id.as_uuid())
        .execute(&db.pool)
        .await?;

    let second = create_invoice(&repo, tenant, customer_id, None, "in_reuse").await?;
    let list = repo.list(tenant).await?;

    assert_eq!(list, vec![second]);
    Ok(())
}

#[tokio::test]
async fn create_with_unknown_subscription_fails_on_fk() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let result = create_invoice(
        &repo,
        tenant,
        customer_id,
        Some(SubscriptionId::new(Uuid::new_v4())),
        "in_fk",
    )
    .await;

    assert!(matches!(result, Err(DomainError::Repository(_))));
    Ok(())
}
