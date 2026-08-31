mod common;

use std::error::Error;

use domain::{DomainError, TenantId, WebhookEventRepository};
use persistence::PgWebhookEventRepository;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn create_then_find_by_stripe_event_id() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgWebhookEventRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let payload = json!({ "id": "evt_1", "type": "invoice.paid" });

    let created = repo
        .create(
            Some(tenant),
            "evt_1".to_string(),
            "invoice.paid".to_string(),
            payload.clone(),
        )
        .await?;
    let found = repo.find_by_stripe_event_id("evt_1").await?;

    assert_eq!(found, Some(created.clone()));
    assert_eq!(created.tenant_id, Some(tenant));
    assert_eq!(created.payload, payload);
    assert_eq!(created.processed_at, None);
    Ok(())
}

#[tokio::test]
async fn create_without_tenant_is_allowed() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgWebhookEventRepository::new(db.pool.clone());

    let created = repo
        .create(
            None,
            "evt_no_tenant".to_string(),
            "customer.subscription.updated".to_string(),
            json!({}),
        )
        .await?;
    let found = repo.find_by_stripe_event_id("evt_no_tenant").await?;

    assert_eq!(found, Some(created.clone()));
    assert_eq!(created.tenant_id, None);
    Ok(())
}

#[tokio::test]
async fn find_by_unknown_stripe_event_id_returns_none() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgWebhookEventRepository::new(db.pool.clone());

    let found = repo.find_by_stripe_event_id("evt_missing").await?;

    assert_eq!(found, None);
    Ok(())
}

#[tokio::test]
async fn duplicate_stripe_event_id_is_rejected() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgWebhookEventRepository::new(db.pool.clone());

    repo.create(
        None,
        "evt_dup".to_string(),
        "invoice.paid".to_string(),
        json!({ "seq": 1 }),
    )
    .await?;
    let second = repo
        .create(
            None,
            "evt_dup".to_string(),
            "invoice.paid".to_string(),
            json!({ "seq": 2 }),
        )
        .await;

    // `create` opts into `RepositoryError::classify`, so a duplicate
    // `stripe_event_id` (SQLSTATE 23505) surfaces as the sharper
    // `Conflict`, not the opaque `Repository`. Phase 3's dedup reads this
    // exact variant as the "already delivered" signal.
    assert!(matches!(second, Err(DomainError::Conflict)));
    Ok(())
}

#[tokio::test]
async fn mark_processed_sets_processed_at() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgWebhookEventRepository::new(db.pool.clone());

    let created = repo
        .create(
            None,
            "evt_mark_processed".to_string(),
            "customer.subscription.updated".to_string(),
            json!({}),
        )
        .await?;
    assert_eq!(created.processed_at, None);

    repo.mark_processed(created.id).await?;

    let found = repo.find_by_stripe_event_id("evt_mark_processed").await?;
    assert!(found.is_some_and(|event| event.processed_at.is_some()));
    Ok(())
}

#[tokio::test]
async fn mark_processed_twice_is_harmless() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgWebhookEventRepository::new(db.pool.clone());

    let created = repo
        .create(
            None,
            "evt_mark_processed_twice".to_string(),
            "customer.subscription.updated".to_string(),
            json!({}),
        )
        .await?;

    repo.mark_processed(created.id).await?;
    // A second call is not an error and does not need the row to still be
    // in some particular state -- decision 5's "still processed even for a
    // declined event" path calls this unconditionally.
    repo.mark_processed(created.id).await?;

    let found = repo
        .find_by_stripe_event_id("evt_mark_processed_twice")
        .await?;
    assert!(found.is_some_and(|event| event.processed_at.is_some()));
    Ok(())
}
