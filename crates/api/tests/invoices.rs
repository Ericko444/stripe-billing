//! Router-level tests for `GET /invoices`: the limit
//! clamp, the opaque cursor, `next` omission on the last page, and tenant
//! isolation.

mod common;

use std::error::Error;

use api::billing_router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use common::{HeaderTenant, StubReads, app_state};
use domain::{Currency, CustomerId, Invoice, InvoiceId, InvoiceStatus, Money, TenantId};
use serde_json::Value;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

fn invoice(tenant: TenantId, seconds_from_epoch: i64) -> Invoice {
    Invoice {
        id: InvoiceId::new(Uuid::new_v4()),
        tenant_id: tenant,
        customer_id: CustomerId::new(Uuid::new_v4()),
        subscription_id: None,
        stripe_invoice_id: format!("in_{}", Uuid::new_v4()),
        amount: Money::new(4200, Currency::Eur),
        status: InvoiceStatus::Paid,
        last_event_created_at: None,
        created_at: OffsetDateTime::UNIX_EPOCH + Duration::seconds(seconds_from_epoch),
        deleted_at: None,
    }
}

async fn get_invoices(
    state: api::AppState,
    tenant: &str,
    query: &str,
) -> Result<(StatusCode, Option<String>, Value), Box<dyn Error>> {
    let uri = if query.is_empty() {
        "/invoices".to_string()
    } else {
        format!("/invoices?{query}")
    };
    let response = billing_router::<HeaderTenant>(state)
        .oneshot(
            Request::get(uri)
                .header("x-tenant", tenant)
                .body(Body::empty())?,
        )
        .await?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Ok((status, content_type, body))
}

fn item_count(body: &Value) -> usize {
    body["items"].as_array().map(Vec::len).unwrap_or(0)
}

async fn get_invoice_by_id(
    state: api::AppState,
    tenant: &str,
    id: &str,
) -> Result<(StatusCode, Option<String>, Value), Box<dyn Error>> {
    let response = billing_router::<HeaderTenant>(state)
        .oneshot(
            Request::get(format!("/invoices/{id}"))
                .header("x-tenant", tenant)
                .body(Body::empty())?,
        )
        .await?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Ok((status, content_type, body))
}

#[tokio::test]
async fn limit_clamps_and_never_rejects() -> Result<(), Box<dyn Error>> {
    let tenant = TenantId::new(Uuid::new_v4());
    let invoices = (0..150).map(|i| invoice(tenant, i)).collect();
    let state = app_state(StubReads {
        invoices,
        ..Default::default()
    });
    let tid = tenant.as_uuid().to_string();

    // Above the max: clamped to 100, not a 400.
    let (status, _, body) = get_invoices(state.clone(), &tid, "limit=1000").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item_count(&body), 100);

    // Non-numeric: treated as absent -> default 25.
    let (status, _, body) = get_invoices(state.clone(), &tid, "limit=garbage").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item_count(&body), 25);

    // Zero: clamped up to 1.
    let (status, _, body) = get_invoices(state.clone(), &tid, "limit=0").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item_count(&body), 1);

    // Absent: default 25.
    let (status, _, body) = get_invoices(state, &tid, "").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item_count(&body), 25);
    Ok(())
}

#[tokio::test]
async fn a_malformed_cursor_is_a_400_problem_json() -> Result<(), Box<dyn Error>> {
    let tenant = TenantId::new(Uuid::new_v4());
    let state = app_state(StubReads {
        invoices: vec![invoice(tenant, 0)],
        ..Default::default()
    });

    let (status, content_type, body) = get_invoices(
        state,
        &tenant.as_uuid().to_string(),
        "after=not-a-real-cursor",
    )
    .await?;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    assert_eq!(body["status"], 400);
    Ok(())
}

#[tokio::test]
async fn next_pages_through_and_is_absent_on_the_last_page() -> Result<(), Box<dyn Error>> {
    let tenant = TenantId::new(Uuid::new_v4());
    let invoices = (0..5).map(|i| invoice(tenant, i)).collect();
    let state = app_state(StubReads {
        invoices,
        ..Default::default()
    });
    let tid = tenant.as_uuid().to_string();

    // Page 1 of 2: a `next` cursor is present.
    let (status, _, page1) = get_invoices(state.clone(), &tid, "limit=3").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item_count(&page1), 3);
    let cursor = page1["next"].as_str().ok_or("page 1 has a next cursor")?;

    // Page 2: the remaining 2 rows, no overlap, and `next` omitted (not null).
    let (status, _, page2) = get_invoices(state, &tid, &format!("limit=3&after={cursor}")).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item_count(&page2), 2);
    assert!(page2.get("next").is_none(), "next omitted on the last page");

    let ids_1: Vec<&str> = page1["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i["id"].as_str())
        .collect();
    let ids_2: Vec<&str> = page2["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i["id"].as_str())
        .collect();
    assert!(ids_1.iter().all(|id| !ids_2.contains(id)), "no overlap");
    Ok(())
}

#[tokio::test]
async fn returns_only_the_calling_tenants_invoices() -> Result<(), Box<dyn Error>> {
    let mine = TenantId::new(Uuid::new_v4());
    let theirs = TenantId::new(Uuid::new_v4());
    let mut invoices: Vec<Invoice> = (0..3).map(|i| invoice(mine, i)).collect();
    let their_invoice = invoice(theirs, 99);
    let their_id = their_invoice.id;
    invoices.push(their_invoice);
    let state = app_state(StubReads {
        invoices,
        ..Default::default()
    });

    let (status, _, body) = get_invoices(state, &mine.as_uuid().to_string(), "limit=100").await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(item_count(&body), 3);
    assert!(
        !body.to_string().contains(&their_id.as_uuid().to_string()),
        "no trace of the other tenant's invoice",
    );
    Ok(())
}

// --- GET /invoices/{id} ------------------------------------------------

#[tokio::test]
async fn get_invoice_returns_the_row_for_its_own_tenant() -> Result<(), Box<dyn Error>> {
    let tenant = TenantId::new(Uuid::new_v4());
    let mut seeded = invoice(tenant, 0);
    seeded.status = InvoiceStatus::Paid;
    let id = seeded.id;
    let state = app_state(StubReads {
        invoices: vec![seeded],
        ..Default::default()
    });

    let (status, _, body) = get_invoice_by_id(
        state,
        &tenant.as_uuid().to_string(),
        &id.as_uuid().to_string(),
    )
    .await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], id.as_uuid().to_string());
    assert_eq!(body["status"], "paid");
    Ok(())
}

#[tokio::test]
async fn unknown_id_and_another_tenants_id_return_an_identical_404() -> Result<(), Box<dyn Error>> {
    let mine = TenantId::new(Uuid::new_v4());
    let theirs = TenantId::new(Uuid::new_v4());
    let their_invoice = invoice(theirs, 0);
    let their_id = their_invoice.id.as_uuid().to_string();
    let state = app_state(StubReads {
        invoices: vec![their_invoice],
        ..Default::default()
    });
    let mine_str = mine.as_uuid().to_string();

    // An id that exists, but for another tenant.
    let (status_other, ct_other, mut body_other) =
        get_invoice_by_id(state.clone(), &mine_str, &their_id).await?;
    // An id that exists nowhere.
    let (status_unknown, ct_unknown, mut body_unknown) =
        get_invoice_by_id(state, &mine_str, &Uuid::new_v4().to_string()).await?;

    assert_eq!(status_other, StatusCode::NOT_FOUND);
    assert_eq!(status_unknown, StatusCode::NOT_FOUND);
    assert_eq!(ct_other.as_deref(), Some("application/problem+json"));
    assert_eq!(ct_unknown, ct_other);

    // Byte-identical once the per-request correlation id is removed.
    for body in [&mut body_other, &mut body_unknown] {
        if let Some(obj) = body.as_object_mut() {
            obj.remove("correlation_id");
        }
    }
    assert_eq!(body_other, body_unknown);
    // And the 404 body names nothing about the other tenant's invoice.
    assert!(!body_other.to_string().contains(&their_id));
    Ok(())
}

#[tokio::test]
async fn a_non_uuid_id_is_a_404_problem_json() -> Result<(), Box<dyn Error>> {
    let tenant = TenantId::new(Uuid::new_v4());
    let state = app_state(StubReads::default());

    let (status, content_type, _) =
        get_invoice_by_id(state, &tenant.as_uuid().to_string(), "not-a-uuid").await?;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    Ok(())
}
