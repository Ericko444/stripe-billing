//! Tasks 18-20: the subscription methods against wiremock + a real Postgres
//! ledger. Idempotency-ledger behaviour itself is covered generically in
//! `idempotency.rs`; here we assert what is specific to each method -- the
//! request bodies it sends and the snapshot it builds.

mod common;

use std::error::Error;

use domain::{CancellationTiming, SubscriptionStatus, TenantId};
use serde_json::json;
use stripe_adapter::StripeBillingProvider;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// A full, deserializable `stripe_shared::Subscription` body with exactly
/// one item. The RC's deserializer requires every non-nullable field to be
/// present -- including the nested `plan` and `price` objects on the item --
/// so this fixture spells all of them out. `current_period_start` /
/// `current_period_end` are on the *item*, which is where the adapter reads
/// the billing period from in this API version.
fn subscription_body(
    sub_id: &str,
    item_id: &str,
    status: &str,
    cancel_at_period_end: bool,
    period_start: i64,
    period_end: i64,
) -> serde_json::Value {
    let mut body = subscription_body_without_items(sub_id, status, cancel_at_period_end);
    body["items"]["data"] = json!([{
        "id": item_id,
        "object": "subscription_item",
        "created": 1_700_000_000,
        "current_period_start": period_start,
        "current_period_end": period_end,
        "discounts": [],
        "metadata": {},
        "subscription": sub_id,
        "plan": {
            "id": "plan_fixture",
            "object": "plan",
            "active": true,
            "billing_scheme": "per_unit",
            "created": 1_700_000_000,
            "currency": "usd",
            "interval": "month",
            "interval_count": 1,
            "livemode": false,
            "usage_type": "licensed",
        },
        "price": {
            "id": "price_fixture",
            "object": "price",
            "active": true,
            "billing_scheme": "per_unit",
            "created": 1_700_000_000,
            "currency": "usd",
            "livemode": false,
            "metadata": {},
            "product": "prod_fixture",
            "type": "recurring",
        },
    }]);
    body
}

/// The same fixture with an empty `items.data` -- the "no subscription item"
/// case Task 18 must turn into an error rather than a blank id.
fn subscription_body_without_items(
    sub_id: &str,
    status: &str,
    cancel_at_period_end: bool,
) -> serde_json::Value {
    json!({
        "id": sub_id,
        "object": "subscription",
        "automatic_tax": { "enabled": false },
        "billing_cycle_anchor": 1_700_000_000,
        "billing_mode": { "type": "classic" },
        "billing_schedules": [],
        "cancel_at_period_end": cancel_at_period_end,
        "collection_method": "charge_automatically",
        "created": 1_700_000_000,
        "currency": "usd",
        "customer": "cus_fixture",
        "invoice_settings": { "issuer": { "type": "self" } },
        "items": {
            "object": "list",
            "data": [],
            "has_more": false,
            "url": "/v1/subscription_items?subscription=sub_fixture",
        },
        "livemode": false,
        "metadata": {},
        "start_date": 1_700_000_000,
        "status": status,
    })
}

fn key_of(req: &wiremock::Request) -> String {
    req.headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

// --- Task 18: create_subscription ------------------------------------------

#[tokio::test]
async fn create_subscription_returns_a_full_snapshot() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    Mock::given(method("POST"))
        .and(path("/v1/subscriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(subscription_body(
            "sub_created",
            "si_created",
            "incomplete",
            false,
            1_700_000_000,
            1_702_678_400,
        )))
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    let snapshot = provider
        .create_subscription(tenant, "cus_created", "price_created")
        .await?;

    assert_eq!(snapshot.stripe_subscription_id, "sub_created");
    assert_eq!(snapshot.stripe_subscription_item_id, "si_created");
    assert_eq!(snapshot.status, SubscriptionStatus::Incomplete);
    assert_eq!(
        snapshot.current_period_start.unix_timestamp(),
        1_700_000_000
    );
    assert_eq!(snapshot.current_period_end.unix_timestamp(), 1_702_678_400);
    assert!(!snapshot.cancel_at_period_end);

    let body = String::from_utf8(
        env.mock_server
            .received_requests()
            .await
            .ok_or("recording on")?[0]
            .body
            .clone(),
    )?;
    assert!(
        body.contains("customer=cus_created"),
        "the customer id must be in the form body: {body}"
    );
    assert!(
        body.contains("items[0][price]=price_created"),
        "the price must be sent as the first item's price: {body}"
    );

    Ok(())
}

#[tokio::test]
async fn create_subscription_with_no_item_is_an_error_not_a_blank_id() -> Result<(), Box<dyn Error>>
{
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    Mock::given(method("POST"))
        .and(path("/v1/subscriptions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(subscription_body_without_items(
                "sub_no_items",
                "incomplete",
                false,
            )),
        )
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    let result = provider
        .create_subscription(tenant, "cus_x", "price_x")
        .await;

    assert!(
        result.is_err(),
        "a subscription response with no items must not yield a snapshot with \
         a blank item id -- that silently breaks change_plan later"
    );

    Ok(())
}

// --- Task 19: change_plan ------------------------------------------------

#[tokio::test]
async fn change_plan_updates_the_stored_item_and_prorates() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    Mock::given(method("POST"))
        .and(path("/v1/subscriptions/sub_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(subscription_body(
            "sub_1",
            "si_stored",
            "active",
            false,
            1_700_000_000,
            1_702_678_400,
        )))
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    provider
        .change_plan(tenant, "sub_1", "si_stored", "price_new")
        .await?;

    let body = String::from_utf8(
        env.mock_server
            .received_requests()
            .await
            .ok_or("recording on")?[0]
            .body
            .clone(),
    )?;
    assert!(
        body.contains("items[0][id]=si_stored"),
        "the request must update the STORED item, not add a new one -- this is \
         the assertion that guards the double-billing bug: {body}"
    );
    assert!(
        body.contains("items[0][price]=price_new"),
        "the new price must be on that same item: {body}"
    );
    assert!(
        body.contains("proration_behavior=create_prorations"),
        "a plan change must prorate: {body}"
    );

    Ok(())
}

// --- Task 20: cancel_subscription --------------------------------------

#[tokio::test]
async fn cancel_at_period_end_updates_the_subscription_and_stays_live() -> Result<(), Box<dyn Error>>
{
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    Mock::given(method("POST"))
        .and(path("/v1/subscriptions/sub_ape"))
        .respond_with(ResponseTemplate::new(200).set_body_json(subscription_body(
            "sub_ape",
            "si_ape",
            "active",
            true,
            1_700_000_000,
            1_702_678_400,
        )))
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    let snapshot = provider
        .cancel_subscription(tenant, "sub_ape", CancellationTiming::AtPeriodEnd)
        .await?;

    assert!(
        snapshot.cancel_at_period_end,
        "AtPeriodEnd must return a snapshot flagged to end at the boundary"
    );
    assert_ne!(
        snapshot.status,
        SubscriptionStatus::Canceled,
        "the subscription is still live until the period ends"
    );

    let body = String::from_utf8(
        env.mock_server
            .received_requests()
            .await
            .ok_or("recording on")?[0]
            .body
            .clone(),
    )?;
    assert!(
        body.contains("cancel_at_period_end=true"),
        "AtPeriodEnd is a subscription update, not a delete: {body}"
    );

    Ok(())
}

#[tokio::test]
async fn immediate_cancellation_deletes_the_subscription() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    Mock::given(method("DELETE"))
        .and(path("/v1/subscriptions/sub_now"))
        .respond_with(ResponseTemplate::new(200).set_body_json(subscription_body(
            "sub_now",
            "si_now",
            "canceled",
            false,
            1_700_000_000,
            1_702_678_400,
        )))
        .expect(1)
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    provider
        .cancel_subscription(tenant, "sub_now", CancellationTiming::Immediate)
        .await?;

    // The matcher above already proves the verb/path; verify() confirms it
    // was hit exactly once.
    env.mock_server.verify().await;
    Ok(())
}

#[tokio::test]
async fn the_two_cancellation_timings_send_different_requests() -> Result<(), Box<dyn Error>> {
    let env = common::setup().await?;
    let tenant = TenantId::new(Uuid::new_v4());

    Mock::given(method("POST"))
        .and(path("/v1/subscriptions/sub_both"))
        .respond_with(ResponseTemplate::new(200).set_body_json(subscription_body(
            "sub_both",
            "si_both",
            "active",
            true,
            1_700_000_000,
            1_702_678_400,
        )))
        .mount(&env.mock_server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v1/subscriptions/sub_both"))
        .respond_with(ResponseTemplate::new(200).set_body_json(subscription_body(
            "sub_both",
            "si_both",
            "canceled",
            false,
            1_700_000_000,
            1_702_678_400,
        )))
        .mount(&env.mock_server)
        .await;

    let provider = StripeBillingProvider::new(&env.stripe_config, env.repo)?;
    provider
        .cancel_subscription(tenant, "sub_both", CancellationTiming::AtPeriodEnd)
        .await?;
    provider
        .cancel_subscription(tenant, "sub_both", CancellationTiming::Immediate)
        .await?;

    let requests = env
        .mock_server
        .received_requests()
        .await
        .ok_or("recording on")?;
    assert_eq!(requests.len(), 2);
    assert_ne!(
        requests[0].method, requests[1].method,
        "the two timings must produce demonstrably different outbound requests"
    );
    assert_ne!(
        key_of(&requests[0]),
        key_of(&requests[1]),
        "each timing is its own operation and its own idempotency key"
    );

    Ok(())
}
