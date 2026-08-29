mod common;

use std::error::Error;

use domain::{CustomerId, CustomerRepository, PaymentMethodRepository, TenantId};
use persistence::{PgCustomerRepository, PgPaymentMethodRepository};
use sqlx::PgPool;
use uuid::Uuid;

async fn seed_customer(pool: &PgPool, tenant: TenantId) -> Result<CustomerId, Box<dyn Error>> {
    let customer = PgCustomerRepository::new(pool.clone())
        .create(tenant, Some(format!("cus_{}", Uuid::new_v4())))
        .await?;
    Ok(customer.id)
}

#[tokio::test]
async fn create_then_find() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgPaymentMethodRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let created = repo
        .create(
            tenant,
            customer_id,
            "pm_1".to_string(),
            "visa".to_string(),
            "4242".to_string(),
            true,
        )
        .await?;
    let found = repo.find(tenant, created.id).await?;

    assert_eq!(found, Some(created.clone()));
    assert!(created.is_default);
    assert_eq!(created.brand, "visa");
    assert_eq!(created.last4, "4242");
    Ok(())
}

#[tokio::test]
async fn list_is_scoped_to_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgPaymentMethodRepository::new(db.pool.clone());
    let tenant_a = TenantId::new(Uuid::new_v4());
    let tenant_b = TenantId::new(Uuid::new_v4());
    let customer_a = seed_customer(&db.pool, tenant_a).await?;
    let customer_b = seed_customer(&db.pool, tenant_b).await?;

    let pm_a = repo
        .create(
            tenant_a,
            customer_a,
            "pm_a".to_string(),
            "visa".to_string(),
            "1111".to_string(),
            false,
        )
        .await?;
    repo.create(
        tenant_b,
        customer_b,
        "pm_b".to_string(),
        "mastercard".to_string(),
        "2222".to_string(),
        false,
    )
    .await?;

    let list_a = repo.list(tenant_a).await?;

    assert_eq!(list_a, vec![pm_a]);
    Ok(())
}

#[tokio::test]
async fn soft_deleted_payment_method_is_excluded_from_reads() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgPaymentMethodRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let created = repo
        .create(
            tenant,
            customer_id,
            "pm_soft_delete".to_string(),
            "visa".to_string(),
            "4242".to_string(),
            false,
        )
        .await?;
    sqlx::query("UPDATE billing.payment_methods SET deleted_at = now() WHERE id = $1")
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
async fn stripe_payment_method_id_can_be_reused_after_soft_delete() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgPaymentMethodRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let first = repo
        .create(
            tenant,
            customer_id,
            "pm_reuse".to_string(),
            "visa".to_string(),
            "4242".to_string(),
            false,
        )
        .await?;
    sqlx::query("UPDATE billing.payment_methods SET deleted_at = now() WHERE id = $1")
        .bind(first.id.as_uuid())
        .execute(&db.pool)
        .await?;

    let second = repo
        .create(
            tenant,
            customer_id,
            "pm_reuse".to_string(),
            "visa".to_string(),
            "4242".to_string(),
            false,
        )
        .await?;
    let list = repo.list(tenant).await?;

    assert_eq!(list, vec![second]);
    Ok(())
}
