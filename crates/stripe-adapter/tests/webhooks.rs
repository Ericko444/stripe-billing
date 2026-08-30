//! Ledger scenarios for `StripeWebhookVerifier` against a real Postgres.
//!
//! Verification-only cases (bad signature, malformed header, multi-`v1`
//! rotation) stay in `webhook_signature`'s unit tests -- the database has
//! nothing to say about them. What is here is what needs the ledger: the
//! `Fresh`/`Duplicate` distinction, and the **zero-rows** guarantee on
//! every rejection path (an error return alone would also be true of an
//! implementation that recorded first and verified after -- the bug worth
//! guarding).

mod common;

use std::error::Error;

use domain::{DomainError, WebhookReceipt, WebhookVerifier};
use serde_json::json;
use sqlx::PgPool;
use stripe_adapter::{StripeWebhookVerifier, test_support};
use time::{Duration, OffsetDateTime};

use common::WEBHOOK_SIGNING_SECRET;

/// A minimal but well-formed Stripe event envelope, as a compact string.
/// The signature is taken over these exact bytes, so tests sign and send
/// the identical `String`.
fn event_json(id: &str, event_type: &str, created: i64) -> String {
    json!({
        "id": id,
        "object": "event",
        "type": event_type,
        "created": created,
        "data": { "object": {} },
    })
    .to_string()
}

/// Rows currently in `billing.webhook_events`.
async fn row_count(pool: &PgPool) -> Result<i64, Box<dyn Error>> {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM billing.webhook_events")
        .fetch_one(pool)
        .await?;
    Ok(n)
}

#[tokio::test]
async fn valid_signature_records_one_fresh_row() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let created = OffsetDateTime::now_utc().unix_timestamp();
    let body = event_json("evt_valid", "invoice.paid", created);
    let header = test_support::signature_header(&body, created, &[WEBHOOK_SIGNING_SECRET]);

    let verifier = StripeWebhookVerifier::new(db.webhook_config, db.webhook_repo);
    let receipt = verifier.verify_and_record(body.as_bytes(), &header).await?;

    assert!(
        matches!(
            &receipt,
            WebhookReceipt::Fresh(event)
                if event.stripe_event_id == "evt_valid" && event.event_type == "invoice.paid"
        ),
        "expected Fresh(evt_valid), got {receipt:?}"
    );
    assert_eq!(row_count(&db.pool).await?, 1);
    Ok(())
}

#[tokio::test]
async fn tampered_payload_writes_zero_rows() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let created = OffsetDateTime::now_utc().unix_timestamp();
    let signed_body = event_json("evt_tamper", "invoice.paid", created);
    let header = test_support::signature_header(&signed_body, created, &[WEBHOOK_SIGNING_SECRET]);
    // Same header, different body: the signature no longer matches.
    let tampered_body = event_json("evt_tamper", "invoice.payment_failed", created);

    let verifier = StripeWebhookVerifier::new(db.webhook_config, db.webhook_repo);
    let result = verifier
        .verify_and_record(tampered_body.as_bytes(), &header)
        .await;

    assert!(matches!(result, Err(DomainError::WebhookVerification)));
    assert_eq!(
        row_count(&db.pool).await?,
        0,
        "a forged payload must not touch the ledger"
    );
    Ok(())
}

#[tokio::test]
async fn expired_timestamp_writes_zero_rows() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    // A correctly-signed delivery whose timestamp is 10 minutes old, well
    // past the 5-minute default tolerance.
    let created = (OffsetDateTime::now_utc() - Duration::minutes(10)).unix_timestamp();
    let body = event_json("evt_expired", "invoice.paid", created);
    let header = test_support::signature_header(&body, created, &[WEBHOOK_SIGNING_SECRET]);

    let verifier = StripeWebhookVerifier::new(db.webhook_config, db.webhook_repo);
    let result = verifier.verify_and_record(body.as_bytes(), &header).await;

    assert!(matches!(result, Err(DomainError::WebhookVerification)));
    assert_eq!(
        row_count(&db.pool).await?,
        0,
        "a stale delivery must not touch the ledger"
    );
    Ok(())
}

#[tokio::test]
async fn same_event_twice_is_fresh_then_duplicate() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let created = OffsetDateTime::now_utc().unix_timestamp();
    let body = event_json("evt_repeat", "invoice.paid", created);
    let header = test_support::signature_header(&body, created, &[WEBHOOK_SIGNING_SECRET]);

    let verifier = StripeWebhookVerifier::new(db.webhook_config, db.webhook_repo);
    let first = verifier.verify_and_record(body.as_bytes(), &header).await?;
    let second = verifier.verify_and_record(body.as_bytes(), &header).await?;

    assert!(matches!(first, WebhookReceipt::Fresh(_)), "got {first:?}");
    assert!(
        matches!(
            &second,
            WebhookReceipt::Duplicate { stripe_event_id } if stripe_event_id == "evt_repeat"
        ),
        "expected Duplicate(evt_repeat), got {second:?}"
    );
    assert_eq!(row_count(&db.pool).await?, 1);
    Ok(())
}

#[tokio::test]
async fn concurrent_deliveries_write_exactly_one_row() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let created = OffsetDateTime::now_utc().unix_timestamp();
    let body = event_json("evt_race", "invoice.paid", created);
    let header = test_support::signature_header(&body, created, &[WEBHOOK_SIGNING_SECRET]);

    let verifier = StripeWebhookVerifier::new(db.webhook_config, db.webhook_repo);
    let (a, b) = tokio::join!(
        verifier.verify_and_record(body.as_bytes(), &header),
        verifier.verify_and_record(body.as_bytes(), &header),
    );
    let receipts = [a?, b?];

    // Assert on the tally and the row count, never on which future won --
    // "the second one lost the race" is a flaky assertion; "exactly one row
    // exists" is the actual invariant.
    let fresh = receipts
        .iter()
        .filter(|r| matches!(r, WebhookReceipt::Fresh(_)))
        .count();
    let duplicate = receipts
        .iter()
        .filter(|r| matches!(r, WebhookReceipt::Duplicate { .. }))
        .count();
    assert_eq!(fresh, 1, "exactly one delivery should be Fresh");
    assert_eq!(duplicate, 1, "exactly one delivery should be Duplicate");
    assert_eq!(row_count(&db.pool).await?, 1);
    Ok(())
}

#[tokio::test]
async fn unknown_event_type_is_recorded_not_rejected() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let created = OffsetDateTime::now_utc().unix_timestamp();
    // A plausibly-future type this code has never heard of.
    let body = event_json("evt_future", "billing.credit_grant.created", created);
    let header = test_support::signature_header(&body, created, &[WEBHOOK_SIGNING_SECRET]);

    let verifier = StripeWebhookVerifier::new(db.webhook_config, db.webhook_repo);
    let receipt = verifier.verify_and_record(body.as_bytes(), &header).await?;

    assert!(
        matches!(
            &receipt,
            WebhookReceipt::Fresh(event) if event.event_type == "billing.credit_grant.created"
        ),
        "an unrecognised type must still be recorded, got {receipt:?}"
    );
    assert_eq!(row_count(&db.pool).await?, 1);
    Ok(())
}
