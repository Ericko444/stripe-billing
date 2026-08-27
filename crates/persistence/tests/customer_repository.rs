mod common;

use std::error::Error;

use domain::{CustomerRepository, TenantId};
use persistence::PgCustomerRepository;
use uuid::Uuid;

#[tokio::test]
async fn create_then_find() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgCustomerRepository::new(db.pool.clone());
    let tenant_id = TenantId::new(Uuid::new_v4());

    let created = repo.create(tenant_id, Some("cus_1".to_string())).await?;
    let found = repo.find(tenant_id, created.id).await?;

    assert_eq!(found, Some(created));
    Ok(())
}

#[tokio::test]
async fn list_is_scoped_to_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgCustomerRepository::new(db.pool.clone());
    let tenant_a = TenantId::new(Uuid::new_v4());
    let tenant_b = TenantId::new(Uuid::new_v4());

    let customer_a = repo.create(tenant_a, Some("cus_a".to_string())).await?;
    repo.create(tenant_b, Some("cus_b".to_string())).await?;

    let list_a = repo.list(tenant_a).await?;

    assert_eq!(list_a, vec![customer_a]);
    Ok(())
}

#[tokio::test]
async fn soft_deleted_customer_is_excluded_from_reads() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgCustomerRepository::new(db.pool.clone());
    let tenant_id = TenantId::new(Uuid::new_v4());

    let created = repo
        .create(tenant_id, Some("cus_soft_delete".to_string()))
        .await?;
    sqlx::query("UPDATE billing.customers SET deleted_at = now() WHERE id = $1")
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
async fn stripe_customer_id_can_be_reused_after_soft_delete() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgCustomerRepository::new(db.pool.clone());
    let tenant_id = TenantId::new(Uuid::new_v4());

    let first = repo
        .create(tenant_id, Some("cus_reuse".to_string()))
        .await?;
    sqlx::query("UPDATE billing.customers SET deleted_at = now() WHERE id = $1")
        .bind(first.id.as_uuid())
        .execute(&db.pool)
        .await?;

    let second = repo
        .create(tenant_id, Some("cus_reuse".to_string()))
        .await?;
    let list = repo.list(tenant_id).await?;

    assert_eq!(list, vec![second]);
    Ok(())
}
