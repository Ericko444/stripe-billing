mod common;

use std::error::Error;

use domain::{CustomerId, CustomerRepository, EventApplication, PaymentMethodRepository, TenantId};
use persistence::{PgCustomerRepository, PgPaymentMethodRepository};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// `now_utc()` truncated to microsecond precision, matching `TIMESTAMPTZ`.
fn now_micros() -> OffsetDateTime {
    let nanos = OffsetDateTime::now_utc().unix_timestamp_nanos();
    let micros = (nanos / 1_000) * 1_000;
    OffsetDateTime::from_unix_timestamp_nanos(micros).unwrap_or_else(|_| OffsetDateTime::now_utc())
}

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

// --- Task 19: find_by_stripe_payment_method_id + apply_event + detach_event ---

#[tokio::test]
async fn find_by_stripe_payment_method_id_is_scoped_to_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgPaymentMethodRepository::new(db.pool.clone());
    let tenant_a = TenantId::new(Uuid::new_v4());
    let tenant_b = TenantId::new(Uuid::new_v4());
    let cust_a = seed_customer(&db.pool, tenant_a).await?;
    let cust_b = seed_customer(&db.pool, tenant_b).await?;

    let a = repo
        .create(
            tenant_a,
            cust_a,
            "pm_shared".into(),
            "visa".into(),
            "1111".into(),
            false,
        )
        .await?;
    let b = repo
        .create(
            tenant_b,
            cust_b,
            "pm_shared".into(),
            "amex".into(),
            "2222".into(),
            false,
        )
        .await?;

    assert_eq!(
        repo.find_by_stripe_payment_method_id(tenant_a, "pm_shared")
            .await?,
        Some(a)
    );
    assert_eq!(
        repo.find_by_stripe_payment_method_id(tenant_b, "pm_shared")
            .await?,
        Some(b)
    );
    assert_eq!(
        repo.find_by_stripe_payment_method_id(tenant_a, "pm_absent")
            .await?,
        None
    );
    Ok(())
}

#[tokio::test]
async fn apply_event_inserts_when_absent_then_updates_with_the_guard() -> Result<(), Box<dyn Error>>
{
    let db = common::setup().await?;
    let repo = PgPaymentMethodRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let t1 = now_micros();
    assert_eq!(
        repo.apply_event(tenant, customer_id, "pm_x", "visa", "4242", true, t1)
            .await?,
        EventApplication::Applied
    );
    let inserted = repo
        .find_by_stripe_payment_method_id(tenant, "pm_x")
        .await?
        .ok_or("inserted")?;
    assert_eq!(inserted.brand, "visa");
    assert_eq!(inserted.last4, "4242");
    assert!(inserted.is_default);
    assert_eq!(inserted.last_event_created_at, Some(t1));

    let t2 = t1 + Duration::minutes(1);
    assert_eq!(
        repo.apply_event(tenant, customer_id, "pm_x", "visa", "9999", false, t2)
            .await?,
        EventApplication::Applied
    );
    let updated = repo
        .find_by_stripe_payment_method_id(tenant, "pm_x")
        .await?
        .ok_or("updated")?;
    assert_eq!(updated.id, inserted.id);
    assert_eq!(updated.last4, "9999");
    assert!(!updated.is_default);
    assert_eq!(updated.last_event_created_at, Some(t2));
    Ok(())
}

#[tokio::test]
async fn apply_event_older_timestamp_is_stale_and_leaves_the_row_unchanged()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgPaymentMethodRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let newer = now_micros();
    let seeded = repo
        .apply_event(tenant, customer_id, "pm_ord", "visa", "4242", true, newer)
        .await?;
    assert_eq!(seeded, EventApplication::Applied);

    let older = newer - Duration::minutes(5);
    assert_eq!(
        repo.apply_event(tenant, customer_id, "pm_ord", "amex", "0000", false, older)
            .await?,
        EventApplication::Stale
    );
    let found = repo
        .find_by_stripe_payment_method_id(tenant, "pm_ord")
        .await?
        .ok_or("row exists")?;
    assert_eq!(found.brand, "visa");
    assert_eq!(found.last4, "4242");
    assert_eq!(found.last_event_created_at, Some(newer));
    Ok(())
}

#[tokio::test]
async fn apply_event_equal_timestamp_applies() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgPaymentMethodRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let t = now_micros();
    let first = repo
        .apply_event(tenant, customer_id, "pm_eq", "visa", "4242", false, t)
        .await?;
    assert_eq!(first, EventApplication::Applied);
    assert_eq!(
        repo.apply_event(tenant, customer_id, "pm_eq", "visa", "5555", false, t)
            .await?,
        EventApplication::Applied
    );
    let found = repo
        .find_by_stripe_payment_method_id(tenant, "pm_eq")
        .await?
        .ok_or("row exists")?;
    assert_eq!(found.last4, "5555");
    Ok(())
}

#[tokio::test]
async fn detach_event_soft_deletes_and_the_guard_still_applies() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgPaymentMethodRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let attached_at = now_micros();
    let attached = repo
        .apply_event(
            tenant,
            customer_id,
            "pm_del",
            "visa",
            "4242",
            true,
            attached_at,
        )
        .await?;
    assert_eq!(attached, EventApplication::Applied);

    // A stale detach -- older than the attach -- must not remove the row.
    let stale = attached_at - Duration::minutes(1);
    assert_eq!(
        repo.detach_event(tenant, "pm_del", stale).await?,
        EventApplication::Stale
    );
    assert!(
        repo.find_by_stripe_payment_method_id(tenant, "pm_del")
            .await?
            .is_some(),
        "stale detach left the row"
    );

    // A newer detach removes it (soft).
    let detached_at = attached_at + Duration::minutes(1);
    assert_eq!(
        repo.detach_event(tenant, "pm_del", detached_at).await?,
        EventApplication::Applied
    );
    assert_eq!(
        repo.find_by_stripe_payment_method_id(tenant, "pm_del")
            .await?,
        None
    );
    Ok(())
}
