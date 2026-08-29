mod common;

use std::error::Error;

use domain::{DomainError, OutboundRequestRepository, TenantId};
use persistence::PgOutboundRequestRepository;
use uuid::Uuid;

#[tokio::test]
async fn create_then_find_by_idempotency_key() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgOutboundRequestRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());

    let created = repo
        .create(
            tenant,
            "create_subscription".to_string(),
            "fp_1".to_string(),
            "idem_1".to_string(),
        )
        .await?;
    let found = repo.find_by_idempotency_key("idem_1").await?;

    assert_eq!(found, Some(created.clone()));
    assert_eq!(created.tenant_id, tenant);
    assert_eq!(created.stripe_object_id, None);
    assert_eq!(created.completed_at, None);
    Ok(())
}

#[tokio::test]
async fn find_by_unknown_idempotency_key_returns_none() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgOutboundRequestRepository::new(db.pool.clone());

    let found = repo.find_by_idempotency_key("idem_missing").await?;

    assert_eq!(found, None);
    Ok(())
}

#[tokio::test]
async fn duplicate_idempotency_key_is_rejected() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgOutboundRequestRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());

    repo.create(
        tenant,
        "create_subscription".to_string(),
        "fp_a".to_string(),
        "idem_dup".to_string(),
    )
    .await?;
    let second = repo
        .create(
            tenant,
            "create_subscription".to_string(),
            "fp_b".to_string(),
            "idem_dup".to_string(),
        )
        .await;

    assert!(matches!(second, Err(DomainError::Repository(_))));
    Ok(())
}
