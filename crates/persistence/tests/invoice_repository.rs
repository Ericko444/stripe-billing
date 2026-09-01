//! Integration tests for `invoice_repository`, against a disposable Postgres.

mod common;

use std::error::Error;

use domain::{
    Currency, CustomerId, CustomerRepository, DomainError, EventApplication, Invoice,
    InvoiceCursor, InvoiceId, InvoiceRepository, InvoiceStatus, Money, PlanRepository,
    SubscriptionId, SubscriptionRepository, SubscriptionStatus, TenantId,
};
use persistence::{
    PgCustomerRepository, PgInvoiceRepository, PgPlanRepository, PgSubscriptionRepository,
};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// `now_utc()` truncated to microsecond precision, matching what `TIMESTAMPTZ`
/// stores -- a raw nanosecond value fails an equality assertion against a row
/// that has round-tripped through Postgres on the sub-microsecond digits alone.
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
    // A row created outside the webhook path has no ordering anchor yet.
    assert_eq!(created.last_event_created_at, None);
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

// --- Task 17: find_by_stripe_invoice_id + apply_event ---

#[tokio::test]
async fn find_by_stripe_invoice_id_is_scoped_to_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant_a = TenantId::new(Uuid::new_v4());
    let tenant_b = TenantId::new(Uuid::new_v4());
    let customer_a = seed_customer(&db.pool, tenant_a).await?;
    let customer_b = seed_customer(&db.pool, tenant_b).await?;

    let invoice_a = create_invoice(&repo, tenant_a, customer_a, None, "in_shared").await?;
    let invoice_b = create_invoice(&repo, tenant_b, customer_b, None, "in_shared").await?;

    // Each tenant sees only its own row, even though the Stripe id collides.
    assert_eq!(
        repo.find_by_stripe_invoice_id(tenant_a, "in_shared")
            .await?,
        Some(invoice_a)
    );
    assert_eq!(
        repo.find_by_stripe_invoice_id(tenant_b, "in_shared")
            .await?,
        Some(invoice_b)
    );
    assert_eq!(
        repo.find_by_stripe_invoice_id(tenant_a, "in_absent")
            .await?,
        None
    );
    Ok(())
}

#[tokio::test]
async fn find_by_stripe_invoice_id_excludes_soft_deleted() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let created = create_invoice(&repo, tenant, customer_id, None, "in_gone").await?;
    sqlx::query("UPDATE billing.invoices SET deleted_at = now() WHERE id = $1")
        .bind(created.id.as_uuid())
        .execute(&db.pool)
        .await?;

    assert_eq!(
        repo.find_by_stripe_invoice_id(tenant, "in_gone").await?,
        None
    );
    Ok(())
}

#[tokio::test]
async fn apply_event_inserts_the_mirror_when_absent() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;
    let subscription_id = seed_subscription(&db.pool, tenant, customer_id).await?;

    let event_created_at = now_micros();
    let outcome = repo
        .apply_event(
            tenant,
            customer_id,
            Some(subscription_id),
            "in_first_seen",
            Money::new(4200, Currency::Eur),
            InvoiceStatus::Paid,
            event_created_at,
        )
        .await?;

    assert_eq!(outcome, EventApplication::Applied);

    let found = repo
        .find_by_stripe_invoice_id(tenant, "in_first_seen")
        .await?
        .ok_or("mirror was inserted")?;
    assert_eq!(found.status, InvoiceStatus::Paid);
    assert_eq!(found.amount, Money::new(4200, Currency::Eur));
    assert_eq!(found.customer_id, customer_id);
    assert_eq!(found.subscription_id, Some(subscription_id));
    assert_eq!(found.last_event_created_at, Some(event_created_at));
    Ok(())
}

#[tokio::test]
async fn apply_event_updates_an_existing_mirror_and_advances_the_ordering_column()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let created = create_invoice(&repo, tenant, customer_id, None, "in_update").await?;
    assert_eq!(created.status, InvoiceStatus::Open);

    let event_created_at = now_micros();
    let outcome = repo
        .apply_event(
            tenant,
            customer_id,
            None,
            "in_update",
            Money::new(4200, Currency::Eur),
            InvoiceStatus::Paid,
            event_created_at,
        )
        .await?;

    assert_eq!(outcome, EventApplication::Applied);

    let found = repo
        .find_by_stripe_invoice_id(tenant, "in_update")
        .await?
        .ok_or("row exists")?;
    assert_eq!(found.id, created.id);
    assert_eq!(found.status, InvoiceStatus::Paid);
    assert_eq!(found.last_event_created_at, Some(event_created_at));
    Ok(())
}

#[tokio::test]
async fn apply_event_with_an_older_timestamp_is_stale_and_leaves_the_row_unchanged()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    let newer_created_at = now_micros();
    let first = repo
        .apply_event(
            tenant,
            customer_id,
            None,
            "in_ordering",
            Money::new(4200, Currency::Eur),
            InvoiceStatus::Paid,
            newer_created_at,
        )
        .await?;
    assert_eq!(first, EventApplication::Applied);

    // An older redelivery carrying a different status and amount -- the guard
    // must refuse to let it overwrite the row written above.
    let older_created_at = newer_created_at - Duration::minutes(5);
    let stale = repo
        .apply_event(
            tenant,
            customer_id,
            None,
            "in_ordering",
            Money::new(9999, Currency::Eur),
            InvoiceStatus::Failed,
            older_created_at,
        )
        .await?;

    assert_eq!(stale, EventApplication::Stale);

    let found = repo
        .find_by_stripe_invoice_id(tenant, "in_ordering")
        .await?
        .ok_or("row exists")?;
    assert_eq!(found.status, InvoiceStatus::Paid);
    assert_eq!(found.amount, Money::new(4200, Currency::Eur));
    assert_eq!(found.last_event_created_at, Some(newer_created_at));
    Ok(())
}

#[tokio::test]
async fn apply_event_with_an_equal_timestamp_applies() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    // Stripe's `created` has second granularity, so two genuine events can
    // share one -- equality must apply, not be dropped as stale.
    let shared = now_micros();
    let first = repo
        .apply_event(
            tenant,
            customer_id,
            None,
            "in_equal",
            Money::new(4200, Currency::Eur),
            InvoiceStatus::Paid,
            shared,
        )
        .await?;
    assert_eq!(first, EventApplication::Applied);

    let second = repo
        .apply_event(
            tenant,
            customer_id,
            None,
            "in_equal",
            Money::new(4200, Currency::Eur),
            InvoiceStatus::Failed,
            shared,
        )
        .await?;

    assert_eq!(second, EventApplication::Applied);
    let found = repo
        .find_by_stripe_invoice_id(tenant, "in_equal")
        .await?
        .ok_or("row exists")?;
    assert_eq!(found.status, InvoiceStatus::Failed);
    Ok(())
}

// --- list_page: keyset pagination (Phase 4b, Task 6) ---------------------

/// Overwrites a row's `created_at`. `create` uses `DEFAULT now()`, so a test
/// that needs a known ordering sets the timestamps itself afterwards.
async fn set_created_at(
    pool: &PgPool,
    id: InvoiceId,
    at: OffsetDateTime,
) -> Result<(), Box<dyn Error>> {
    sqlx::query("UPDATE billing.invoices SET created_at = $2 WHERE id = $1")
        .bind(id.as_uuid())
        .bind(at)
        .execute(pool)
        .await?;
    Ok(())
}

/// Creates `count` invoices for `tenant`, one second apart starting at
/// `base`, and returns their ids in the order `list_page` should yield them:
/// newest `created_at` first, `id` descending to break ties.
async fn seed_invoices_over_time(
    repo: &PgInvoiceRepository,
    pool: &PgPool,
    tenant: TenantId,
    customer_id: CustomerId,
    base: OffsetDateTime,
    count: usize,
) -> Result<Vec<InvoiceId>, Box<dyn Error>> {
    let mut rows = Vec::new();
    for i in 0..count {
        let invoice = create_invoice(
            repo,
            tenant,
            customer_id,
            None,
            &format!("in_pg_{i}_{}", Uuid::new_v4()),
        )
        .await?;
        let at = base + Duration::seconds(i64::try_from(i)?);
        set_created_at(pool, invoice.id, at).await?;
        rows.push((at, invoice.id));
    }
    rows.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.as_uuid().cmp(&a.1.as_uuid()))
    });
    Ok(rows.into_iter().map(|(_, id)| id).collect())
}

/// Walks every page with `limit` and returns the ids in the order paging
/// produced them.
async fn all_ids_via_paging(
    repo: &PgInvoiceRepository,
    tenant: TenantId,
    limit: u16,
) -> Result<Vec<InvoiceId>, Box<dyn Error>> {
    let mut ids = Vec::new();
    let mut after: Option<InvoiceCursor> = None;
    loop {
        let page = repo.list_page(tenant, after, limit).await?;
        assert!(page.items.len() <= usize::from(limit), "page overran limit");
        ids.extend(page.items.iter().map(|invoice| invoice.id));
        match page.next {
            Some(cursor) => after = Some(cursor),
            None => break,
        }
    }
    Ok(ids)
}

#[tokio::test]
async fn list_page_returns_newest_first_and_honours_limit() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;
    let expected =
        seed_invoices_over_time(&repo, &db.pool, tenant, customer_id, now_micros(), 5).await?;

    let page = repo.list_page(tenant, None, 3).await?;

    assert_eq!(page.items.len(), 3);
    assert_eq!(
        page.items.iter().map(|i| i.id).collect::<Vec<_>>(),
        expected[..3].to_vec(),
    );
    assert!(
        page.next.is_some(),
        "a full page with rows behind it has a next cursor"
    );
    Ok(())
}

#[tokio::test]
async fn paging_covers_every_row_with_no_overlap_or_gap() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;
    let expected =
        seed_invoices_over_time(&repo, &db.pool, tenant, customer_id, now_micros(), 10).await?;

    // A limit that does not divide the row count, so the last page is short.
    let paged = all_ids_via_paging(&repo, tenant, 3).await?;

    assert_eq!(paged, expected);
    Ok(())
}

#[tokio::test]
async fn last_full_page_has_no_next_cursor() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;
    seed_invoices_over_time(&repo, &db.pool, tenant, customer_id, now_micros(), 6).await?;

    // Exactly two pages of 3. The second must report `next == None` rather
    // than a cursor that would fetch an empty third page.
    let first = repo.list_page(tenant, None, 3).await?;
    let second = repo.list_page(tenant, first.next, 3).await?;

    assert_eq!(second.items.len(), 3);
    assert!(second.next.is_none());
    Ok(())
}

#[tokio::test]
async fn a_row_inserted_between_pages_does_not_shift_page_two() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;
    let base = now_micros();
    let expected = seed_invoices_over_time(&repo, &db.pool, tenant, customer_id, base, 6).await?;

    let page_one = repo.list_page(tenant, None, 3).await?;
    assert_eq!(
        page_one.items.iter().map(|i| i.id).collect::<Vec<_>>(),
        expected[..3].to_vec(),
    );

    // A new invoice lands newer than anything on page one.
    let intruder = create_invoice(&repo, tenant, customer_id, None, "in_intruder").await?;
    set_created_at(&db.pool, intruder.id, base + Duration::seconds(100)).await?;

    let page_two = repo.list_page(tenant, page_one.next, 3).await?;

    // Page two is still the original rows 4-6; the intruder is not among them.
    assert_eq!(
        page_two.items.iter().map(|i| i.id).collect::<Vec<_>>(),
        expected[3..].to_vec(),
    );
    assert!(page_two.items.iter().all(|i| i.id != intruder.id));
    Ok(())
}

#[tokio::test]
async fn rows_with_identical_created_at_are_not_dropped_or_duplicated() -> Result<(), Box<dyn Error>>
{
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;

    // Seven invoices, all sharing one `created_at` -- the id is the only
    // thing that orders them, which is exactly what the composite cursor is
    // for.
    let shared = now_micros();
    let mut created = Vec::new();
    for i in 0..7 {
        let invoice =
            create_invoice(&repo, tenant, customer_id, None, &format!("in_tie_{i}")).await?;
        set_created_at(&db.pool, invoice.id, shared).await?;
        created.push(invoice.id);
    }

    let paged = all_ids_via_paging(&repo, tenant, 2).await?;

    let mut sorted_paged = paged.clone();
    sorted_paged.sort_by_key(|id| id.as_uuid());
    created.sort_by_key(|id| id.as_uuid());
    assert_eq!(sorted_paged, created, "every tied row appears exactly once");
    assert_eq!(paged.len(), 7);
    Ok(())
}

#[tokio::test]
async fn list_page_is_scoped_to_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let mine = TenantId::new(Uuid::new_v4());
    let theirs = TenantId::new(Uuid::new_v4());
    let my_customer = seed_customer(&db.pool, mine).await?;
    let their_customer = seed_customer(&db.pool, theirs).await?;
    let my_ids =
        seed_invoices_over_time(&repo, &db.pool, mine, my_customer, now_micros(), 4).await?;
    seed_invoices_over_time(&repo, &db.pool, theirs, their_customer, now_micros(), 4).await?;

    let paged = all_ids_via_paging(&repo, mine, 2).await?;

    assert_eq!(paged, my_ids);
    Ok(())
}

#[tokio::test]
async fn list_page_excludes_soft_deleted() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgInvoiceRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let customer_id = seed_customer(&db.pool, tenant).await?;
    let ids =
        seed_invoices_over_time(&repo, &db.pool, tenant, customer_id, now_micros(), 5).await?;

    sqlx::query("UPDATE billing.invoices SET deleted_at = now() WHERE id = $1")
        .bind(ids[2].as_uuid())
        .execute(&db.pool)
        .await?;

    let paged = all_ids_via_paging(&repo, tenant, 2).await?;

    let expected: Vec<InvoiceId> = ids.iter().copied().filter(|id| *id != ids[2]).collect();
    assert_eq!(paged, expected);
    Ok(())
}
