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

    assert!(matches!(second, Err(DomainError::Conflict)));
    Ok(())
}

#[tokio::test]
async fn find_by_fingerprint_returns_most_recent_row() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgOutboundRequestRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());

    let first = repo
        .create(
            tenant,
            "create_subscription".to_string(),
            "fp_recur".to_string(),
            "idem_first".to_string(),
        )
        .await?;
    // The partial unique index only blocks a second *incomplete* row for the
    // same fingerprint, so the first attempt has to complete before a second
    // one can be recorded -- this is what "fingerprint recurs after the key
    // window" looks like at the storage layer.
    repo.mark_complete(tenant, first.id, "obj_first".to_string())
        .await?;
    let second = repo
        .create(
            tenant,
            "create_subscription".to_string(),
            "fp_recur".to_string(),
            "idem_second".to_string(),
        )
        .await?;

    let found = repo
        .find_by_fingerprint(tenant, "create_subscription", "fp_recur")
        .await?;

    assert_eq!(found, Some(second));
    Ok(())
}

#[tokio::test]
async fn find_by_fingerprint_is_scoped_to_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgOutboundRequestRepository::new(db.pool.clone());
    let tenant_a = TenantId::new(Uuid::new_v4());
    let tenant_b = TenantId::new(Uuid::new_v4());

    repo.create(
        tenant_a,
        "create_subscription".to_string(),
        "fp_shared".to_string(),
        "idem_tenant_a".to_string(),
    )
    .await?;

    let found_for_b = repo
        .find_by_fingerprint(tenant_b, "create_subscription", "fp_shared")
        .await?;

    assert_eq!(
        found_for_b, None,
        "tenant B must not see tenant A's outbound request"
    );
    Ok(())
}

#[tokio::test]
async fn mark_complete_sets_completed_at_and_stripe_object_id() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgOutboundRequestRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());

    let created = repo
        .create(
            tenant,
            "create_customer".to_string(),
            "fp_complete".to_string(),
            "idem_complete".to_string(),
        )
        .await?;
    assert_eq!(created.completed_at, None);

    let completed = repo
        .mark_complete(tenant, created.id, "cus_123".to_string())
        .await?;

    assert_eq!(completed.stripe_object_id, Some("cus_123".to_string()));
    assert!(completed.completed_at.is_some());
    Ok(())
}

#[tokio::test]
async fn partial_unique_index_blocks_concurrent_incomplete_rows_but_allows_reuse_after_complete()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgOutboundRequestRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());

    let first = repo
        .create(
            tenant,
            "create_subscription".to_string(),
            "fp_race".to_string(),
            "idem_race_1".to_string(),
        )
        .await?;

    // Same (tenant, operation, fingerprint), still incomplete: the partial
    // unique index must reject this -- it's the guard on the reserve race
    // (migrations/0009_outbound_request_fingerprint.sql).
    let second_while_incomplete = repo
        .create(
            tenant,
            "create_subscription".to_string(),
            "fp_race".to_string(),
            "idem_race_2".to_string(),
        )
        .await;
    assert!(matches!(
        second_while_incomplete,
        Err(DomainError::Conflict)
    ));

    // Once the first row completes, the same fingerprint is free to insert
    // again -- a plain (non-partial) unique index would wrongly block this
    // too, which is exactly the bug the WHERE clause exists to avoid.
    repo.mark_complete(tenant, first.id, "sub_first".to_string())
        .await?;
    let second_after_complete = repo
        .create(
            tenant,
            "create_subscription".to_string(),
            "fp_race".to_string(),
            "idem_race_3".to_string(),
        )
        .await;
    assert!(second_after_complete.is_ok());

    Ok(())
}
