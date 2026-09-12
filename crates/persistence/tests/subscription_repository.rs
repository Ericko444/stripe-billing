//! Integration tests for `subscription_repository`, against a disposable Postgres.

mod common;

use std::error::Error;

use audit::{
    Action, Actor, AuditEntry, CorrelationId, Target, TargetId, TenantId as AuditTenantId,
};
use domain::{
    Currency, CustomerId, CustomerRepository, DomainError, EventApplication, Money, PlanId,
    PlanRepository, Subscription, SubscriptionRepository, SubscriptionSnapshot, SubscriptionStatus,
    TenantId,
};
use persistence::{PgCustomerRepository, PgPlanRepository, PgSubscriptionRepository};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// An `AuditEntry` for `tenant`/`target_id`/`action`, with a fresh,
/// otherwise unremarkable correlation id.
fn entry(tenant: TenantId, target_id: Uuid, action: Action) -> AuditEntry {
    AuditEntry::new(
        AuditTenantId::new(tenant.as_uuid()),
        Actor::System,
        action,
        Target::Subscription(TargetId::new(target_id)),
        OffsetDateTime::now_utc(),
        CorrelationId::new(Uuid::new_v4()),
    )
}

/// Installs a `BEFORE UPDATE` trigger on `billing.subscriptions` that raises
/// for exactly one sentinel `stripe_subscription_id` -- the same technique
/// `payment_method_repository.rs`'s tests use, aimed at the merged
/// `change_plan` transaction's business-write half.
async fn fail_subscriptions_update_for(
    pool: &PgPool,
    sentinel_stripe_subscription_id: &str,
) -> Result<(), Box<dyn Error>> {
    // The interpolated value is a test-chosen literal, never external input
    // -- `AssertSqlSafe` is sqlx 0.9's required, explicit acknowledgement of
    // that.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE OR REPLACE FUNCTION billing.fail_on_sentinel() RETURNS trigger AS $$
         BEGIN
           IF NEW.stripe_subscription_id = '{sentinel_stripe_subscription_id}' THEN
             RAISE EXCEPTION 'test-forced subscription update failure';
           END IF;
           RETURN NEW;
         END;
         $$ LANGUAGE plpgsql;
         CREATE TRIGGER fail_on_sentinel_trigger
           BEFORE UPDATE ON billing.subscriptions
           FOR EACH ROW EXECUTE FUNCTION billing.fail_on_sentinel();"
    )))
    .execute(pool)
    .await?;
    Ok(())
}

/// Same technique, aimed at the audit half of the transaction.
async fn fail_audit_insert_for(
    pool: &PgPool,
    sentinel_target_id: Uuid,
) -> Result<(), Box<dyn Error>> {
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE OR REPLACE FUNCTION audit.fail_on_sentinel() RETURNS trigger AS $$
         BEGIN
           IF NEW.target_id = '{sentinel_target_id}' THEN
             RAISE EXCEPTION 'test-forced audit insert failure';
           END IF;
           RETURN NEW;
         END;
         $$ LANGUAGE plpgsql;
         CREATE TRIGGER fail_on_sentinel_trigger
           BEFORE INSERT ON audit.audit_log
           FOR EACH ROW EXECUTE FUNCTION audit.fail_on_sentinel();"
    )))
    .execute(pool)
    .await?;
    Ok(())
}

/// `now_utc()` truncated to microsecond precision, matching what
/// `TIMESTAMPTZ` actually stores. Using the raw nanosecond-precision value in
/// an equality assertion against a row that has round-tripped through
/// Postgres fails on the sub-microsecond digits alone -- this is what the
/// value going *in* has to look like for that comparison to be meaningful.
fn now_micros() -> OffsetDateTime {
    let nanos = OffsetDateTime::now_utc().unix_timestamp_nanos();
    let micros = (nanos / 1_000) * 1_000;
    OffsetDateTime::from_unix_timestamp_nanos(micros).unwrap_or_else(|_| OffsetDateTime::now_utc())
}

/// Inserts a customer and a plan for `tenant` and returns their ids, so a
/// subscription's FKs point at real rows.
async fn seed(pool: &PgPool, tenant: TenantId) -> Result<(CustomerId, PlanId), Box<dyn Error>> {
    let customer = PgCustomerRepository::new(pool.clone())
        .create(tenant, Some(format!("cus_{}", Uuid::new_v4())))
        .await?;
    let plan = PgPlanRepository::new(pool.clone())
        .create(
            tenant,
            format!("price_{}", Uuid::new_v4()),
            format!("prod_{}", Uuid::new_v4()),
            "Seed plan".to_string(),
            Money::new(1000, Currency::Usd),
        )
        .await?;
    Ok((customer.id, plan.id))
}

async fn create_subscription(
    repo: &PgSubscriptionRepository,
    tenant: TenantId,
    customer_id: CustomerId,
    plan_id: PlanId,
    stripe_subscription_id: &str,
) -> Result<Subscription, DomainError> {
    let now = OffsetDateTime::now_utc();
    repo.create(
        tenant,
        customer_id,
        plan_id,
        stripe_subscription_id.to_string(),
        format!("si_{stripe_subscription_id}"),
        SubscriptionStatus::Active,
        now,
        now + Duration::days(30),
    )
    .await
}

#[tokio::test]
async fn create_then_find() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;

    let created = create_subscription(&repo, tenant, customer_id, plan_id, "sub_1").await?;
    let found = repo.find(tenant, created.id).await?;

    assert_eq!(found, Some(created.clone()));
    assert_eq!(created.status, SubscriptionStatus::Active);
    assert!(!created.cancel_at_period_end);
    assert_eq!(created.last_event_created_at, None);
    Ok(())
}

#[tokio::test]
async fn list_is_scoped_to_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant_a = TenantId::new(Uuid::new_v4());
    let tenant_b = TenantId::new(Uuid::new_v4());
    let (customer_a, plan_a) = seed(&db.pool, tenant_a).await?;
    let (customer_b, plan_b) = seed(&db.pool, tenant_b).await?;

    let sub_a = create_subscription(&repo, tenant_a, customer_a, plan_a, "sub_a").await?;
    create_subscription(&repo, tenant_b, customer_b, plan_b, "sub_b").await?;

    let list_a = repo.list(tenant_a).await?;

    assert_eq!(list_a, vec![sub_a]);
    Ok(())
}

#[tokio::test]
async fn soft_deleted_subscription_is_excluded_from_reads() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;

    let created =
        create_subscription(&repo, tenant, customer_id, plan_id, "sub_soft_delete").await?;
    sqlx::query("UPDATE billing.subscriptions SET deleted_at = now() WHERE id = $1")
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
async fn stripe_subscription_id_can_be_reused_after_soft_delete() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;

    let first = create_subscription(&repo, tenant, customer_id, plan_id, "sub_reuse").await?;
    sqlx::query("UPDATE billing.subscriptions SET deleted_at = now() WHERE id = $1")
        .bind(first.id.as_uuid())
        .execute(&db.pool)
        .await?;

    let second = create_subscription(&repo, tenant, customer_id, plan_id, "sub_reuse").await?;
    let list = repo.list(tenant).await?;

    assert_eq!(list, vec![second]);
    Ok(())
}

#[tokio::test]
async fn create_with_unknown_customer_fails_on_fk() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (_customer_id, plan_id) = seed(&db.pool, tenant).await?;

    let result = create_subscription(
        &repo,
        tenant,
        CustomerId::new(Uuid::new_v4()),
        plan_id,
        "sub_fk",
    )
    .await;

    assert!(matches!(result, Err(DomainError::Repository(_))));
    Ok(())
}

#[tokio::test]
async fn find_by_stripe_subscription_id_is_scoped_to_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant_a = TenantId::new(Uuid::new_v4());
    let tenant_b = TenantId::new(Uuid::new_v4());
    let (customer_a, plan_a) = seed(&db.pool, tenant_a).await?;

    let created =
        create_subscription(&repo, tenant_a, customer_a, plan_a, "sub_find_by_stripe_id").await?;

    let found_own_tenant = repo
        .find_by_stripe_subscription_id(tenant_a, "sub_find_by_stripe_id")
        .await?;
    let found_other_tenant = repo
        .find_by_stripe_subscription_id(tenant_b, "sub_find_by_stripe_id")
        .await?;
    let found_unknown_id = repo
        .find_by_stripe_subscription_id(tenant_a, "sub_never_created")
        .await?;

    assert_eq!(found_own_tenant, Some(created));
    assert_eq!(found_other_tenant, None);
    assert_eq!(found_unknown_id, None);
    Ok(())
}

#[tokio::test]
async fn apply_event_updates_the_row_and_advances_the_ordering_column() -> Result<(), Box<dyn Error>>
{
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created =
        create_subscription(&repo, tenant, customer_id, plan_id, "sub_apply_event").await?;

    let event_created_at = now_micros();
    let new_period_start = event_created_at;
    let new_period_end = event_created_at + Duration::days(30);

    let outcome = repo
        .apply_event(
            tenant,
            created.id,
            SubscriptionStatus::PastDue,
            new_period_start,
            new_period_end,
            true,
            event_created_at,
            entry(tenant, created.id.as_uuid(), Action::SubscriptionUpdated),
        )
        .await?;

    assert_eq!(outcome, EventApplication::Applied);

    let found = repo.find(tenant, created.id).await?.ok_or("row exists")?;
    assert_eq!(found.status, SubscriptionStatus::PastDue);
    assert_eq!(found.current_period_start, new_period_start);
    assert_eq!(found.current_period_end, new_period_end);
    assert!(found.cancel_at_period_end);
    assert_eq!(found.last_event_created_at, Some(event_created_at));
    // apply_event never touches plan_id -- it isn't a parameter this method
    // accepts, and the row should still point at the plan it was created with.
    assert_eq!(found.plan_id, created.plan_id);
    Ok(())
}

#[tokio::test]
async fn apply_event_with_an_older_timestamp_is_stale_and_leaves_the_row_unchanged()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created = create_subscription(&repo, tenant, customer_id, plan_id, "sub_stale").await?;

    let newer_created_at = now_micros();
    let newer_period_start = newer_created_at;
    let newer_period_end = newer_created_at + Duration::days(30);
    let first_outcome = repo
        .apply_event(
            tenant,
            created.id,
            SubscriptionStatus::Active,
            newer_period_start,
            newer_period_end,
            false,
            newer_created_at,
            entry(tenant, created.id.as_uuid(), Action::SubscriptionUpdated),
        )
        .await?;
    assert_eq!(first_outcome, EventApplication::Applied);

    // An older, out-of-order redelivery: an earlier `created`, a different
    // status, different periods -- everything the guard should refuse to
    // let overwrite the row just written above.
    let older_created_at = newer_created_at - Duration::minutes(5);
    let stale_outcome = repo
        .apply_event(
            tenant,
            created.id,
            SubscriptionStatus::Canceled,
            older_created_at,
            older_created_at + Duration::days(1),
            true,
            older_created_at,
            entry(tenant, created.id.as_uuid(), Action::SubscriptionUpdated),
        )
        .await?;

    assert_eq!(stale_outcome, EventApplication::Stale);

    let found = repo.find(tenant, created.id).await?.ok_or("row exists")?;
    assert_eq!(found.status, SubscriptionStatus::Active);
    assert_eq!(found.current_period_start, newer_period_start);
    assert_eq!(found.current_period_end, newer_period_end);
    assert!(!found.cancel_at_period_end);
    assert_eq!(found.last_event_created_at, Some(newer_created_at));
    Ok(())
}

#[tokio::test]
async fn apply_event_with_an_equal_timestamp_applies() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created = create_subscription(&repo, tenant, customer_id, plan_id, "sub_equal").await?;

    // Stripe's `created` has second granularity, so two genuine events can
    // legitimately share one -- equality must apply, not be treated as
    // stale, or the second of the two would be silently dropped.
    let shared_created_at = now_micros();
    let first_outcome = repo
        .apply_event(
            tenant,
            created.id,
            SubscriptionStatus::Active,
            shared_created_at,
            shared_created_at + Duration::days(30),
            false,
            shared_created_at,
            entry(tenant, created.id.as_uuid(), Action::SubscriptionUpdated),
        )
        .await?;
    assert_eq!(first_outcome, EventApplication::Applied);

    let second_outcome = repo
        .apply_event(
            tenant,
            created.id,
            SubscriptionStatus::PastDue,
            shared_created_at,
            shared_created_at + Duration::days(30),
            true,
            shared_created_at,
            entry(tenant, created.id.as_uuid(), Action::SubscriptionUpdated),
        )
        .await?;

    assert_eq!(second_outcome, EventApplication::Applied);

    let found = repo.find(tenant, created.id).await?.ok_or("row exists")?;
    assert_eq!(found.status, SubscriptionStatus::PastDue);
    assert!(found.cancel_at_period_end);
    Ok(())
}

#[tokio::test]
async fn set_plan_repoints_the_row_and_touches_nothing_else() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created = create_subscription(&repo, tenant, customer_id, plan_id, "sub_set_plan").await?;

    // A second plan for the same tenant to move to.
    let (_, other_plan_id) = seed(&db.pool, tenant).await?;

    repo.set_plan(tenant, created.id, other_plan_id).await?;

    let found = repo.find(tenant, created.id).await?.ok_or("row exists")?;
    assert_eq!(found.plan_id, other_plan_id);
    // Every other column is the statement's business to leave alone: this
    // write answers to the caller's plan choice, not to a Stripe event.
    assert_eq!(found.status, created.status);
    assert_eq!(found.current_period_start, created.current_period_start);
    assert_eq!(found.current_period_end, created.current_period_end);
    assert_eq!(found.cancel_at_period_end, created.cancel_at_period_end);
    assert_eq!(found.last_event_created_at, created.last_event_created_at);
    Ok(())
}

#[tokio::test]
async fn set_plan_is_scoped_to_tenant() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let owner = TenantId::new(Uuid::new_v4());
    let intruder = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, owner).await?;
    let created = create_subscription(&repo, owner, customer_id, plan_id, "sub_scoped").await?;
    let (_, intruder_plan_id) = seed(&db.pool, intruder).await?;

    // Another tenant naming a real subscription id must move nothing.
    repo.set_plan(intruder, created.id, intruder_plan_id)
        .await?;

    let found = repo.find(owner, created.id).await?.ok_or("row exists")?;
    assert_eq!(
        found.plan_id, plan_id,
        "a cross-tenant set_plan must not repoint another tenant's subscription"
    );
    Ok(())
}

#[tokio::test]
async fn set_plan_does_not_resurrect_a_soft_deleted_row() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created = create_subscription(&repo, tenant, customer_id, plan_id, "sub_deleted").await?;
    let (_, other_plan_id) = seed(&db.pool, tenant).await?;

    sqlx::query("UPDATE billing.subscriptions SET deleted_at = now() WHERE id = $1")
        .bind(created.id.as_uuid())
        .execute(&db.pool)
        .await?;

    // Not an error -- the documented precondition is that the caller already
    // found the row -- but it must write nothing.
    repo.set_plan(tenant, created.id, other_plan_id).await?;

    let row_plan_id: Uuid =
        sqlx::query_scalar("SELECT plan_id FROM billing.subscriptions WHERE id = $1")
            .bind(created.id.as_uuid())
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(row_plan_id, plan_id.as_uuid());
    Ok(())
}

fn snapshot_of(sub: &Subscription) -> SubscriptionSnapshot {
    SubscriptionSnapshot {
        stripe_subscription_id: sub.stripe_subscription_id.clone(),
        stripe_subscription_item_id: sub.stripe_subscription_item_id.clone(),
        status: sub.status,
        current_period_start: sub.current_period_start,
        current_period_end: sub.current_period_end,
        cancel_at_period_end: sub.cancel_at_period_end,
    }
}

#[tokio::test]
async fn change_plan_repoints_applies_the_snapshot_and_writes_one_audit_row()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created =
        create_subscription(&repo, tenant, customer_id, plan_id, "sub_change_plan").await?;
    let (_, new_plan_id) = seed(&db.pool, tenant).await?;
    let mut snapshot = snapshot_of(&created);
    snapshot.status = SubscriptionStatus::PastDue;
    let audit_entry = entry(
        tenant,
        created.id.as_uuid(),
        Action::SubscriptionPlanChanged,
    );
    let correlation_id = audit_entry.correlation_id().as_uuid();

    let updated = repo
        .change_plan(
            tenant,
            created.id,
            new_plan_id,
            snapshot,
            now_micros(),
            audit_entry,
        )
        .await?;

    assert_eq!(updated.plan_id, new_plan_id);
    assert_eq!(updated.status, SubscriptionStatus::PastDue);

    let audit_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit.audit_log WHERE correlation_id = $1")
            .bind(correlation_id)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(audit_rows, 1);
    let (action, target_id): (String, Option<Uuid>) =
        sqlx::query_as("SELECT action, target_id FROM audit.audit_log WHERE correlation_id = $1")
            .bind(correlation_id)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(action, "subscription.plan_changed");
    assert_eq!(target_id, Some(created.id.as_uuid()));
    Ok(())
}

#[tokio::test]
async fn change_plan_still_moves_the_plan_and_audits_when_the_snapshot_is_stale()
-> Result<(), Box<dyn Error>> {
    // A newer webhook already applied an event to this row -- the same
    // scenario `change_plan_does_not_regress_a_row_a_newer_event_already_applied`
    // exercises at the service layer. The snapshot half of change_plan must
    // be rejected by the ordering guard, but the plan repoint and the audit
    // entry are unguarded and must still happen.
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created = create_subscription(&repo, tenant, customer_id, plan_id, "sub_stale").await?;
    let future = now_micros() + Duration::hours(1);
    let _ = repo
        .apply_event(
            tenant,
            created.id,
            SubscriptionStatus::Active,
            created.current_period_start,
            created.current_period_end,
            created.cancel_at_period_end,
            future,
            entry(tenant, created.id.as_uuid(), Action::SubscriptionUpdated),
        )
        .await?;
    let (_, new_plan_id) = seed(&db.pool, tenant).await?;
    let mut stale_snapshot = snapshot_of(&created);
    stale_snapshot.status = SubscriptionStatus::PastDue;
    let audit_entry = entry(
        tenant,
        created.id.as_uuid(),
        Action::SubscriptionPlanChanged,
    );
    let correlation_id = audit_entry.correlation_id().as_uuid();

    let updated = repo
        .change_plan(
            tenant,
            created.id,
            new_plan_id,
            stale_snapshot,
            now_micros(),
            audit_entry,
        )
        .await?;

    assert_eq!(
        updated.plan_id, new_plan_id,
        "the plan repoint is unguarded and must apply regardless"
    );
    assert_eq!(
        updated.status,
        SubscriptionStatus::Active,
        "the stale snapshot must not regress the status the newer event set"
    );
    let audit_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit.audit_log WHERE correlation_id = $1")
            .bind(correlation_id)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(
        audit_rows, 1,
        "the request happened and is audited regardless of the guard's outcome"
    );
    Ok(())
}

#[tokio::test]
async fn change_plan_rolls_back_both_writes_when_the_audit_insert_fails()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created =
        create_subscription(&repo, tenant, customer_id, plan_id, "sub_audit_fails").await?;
    let (_, new_plan_id) = seed(&db.pool, tenant).await?;
    fail_audit_insert_for(&db.pool, created.id.as_uuid()).await?;
    let mut snapshot = snapshot_of(&created);
    snapshot.status = SubscriptionStatus::PastDue;

    let result = repo
        .change_plan(
            tenant,
            created.id,
            new_plan_id,
            snapshot,
            now_micros(),
            entry(
                tenant,
                created.id.as_uuid(),
                Action::SubscriptionPlanChanged,
            ),
        )
        .await;

    assert!(result.is_err());
    let after = repo
        .find(tenant, created.id)
        .await?
        .ok_or("row still exists")?;
    assert_eq!(
        after.plan_id, plan_id,
        "a failed audit insert must leave the plan repoint rolled back"
    );
    assert_eq!(
        after.status, created.status,
        "a failed audit insert must leave the snapshot apply rolled back too"
    );
    Ok(())
}

#[tokio::test]
async fn change_plan_writes_no_audit_row_when_the_business_write_fails()
-> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created =
        create_subscription(&repo, tenant, customer_id, plan_id, "sub_business_fails").await?;
    let (_, new_plan_id) = seed(&db.pool, tenant).await?;
    fail_subscriptions_update_for(&db.pool, "sub_business_fails").await?;
    let mut snapshot = snapshot_of(&created);
    snapshot.status = SubscriptionStatus::PastDue;
    let audit_entry = entry(
        tenant,
        created.id.as_uuid(),
        Action::SubscriptionPlanChanged,
    );
    let correlation_id = audit_entry.correlation_id().as_uuid();

    let result = repo
        .change_plan(
            tenant,
            created.id,
            new_plan_id,
            snapshot,
            now_micros(),
            audit_entry,
        )
        .await;

    assert!(result.is_err());
    let audit_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit.audit_log WHERE correlation_id = $1")
            .bind(correlation_id)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(audit_rows, 0);
    Ok(())
}

#[tokio::test]
async fn cancel_applies_the_snapshot_and_writes_one_audit_row() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created = create_subscription(&repo, tenant, customer_id, plan_id, "sub_cancel").await?;
    let mut snapshot = snapshot_of(&created);
    snapshot.status = SubscriptionStatus::Canceled;
    let audit_entry = entry(tenant, created.id.as_uuid(), Action::SubscriptionCanceled);
    let correlation_id = audit_entry.correlation_id().as_uuid();

    let updated = repo
        .cancel(tenant, created.id, snapshot, now_micros(), audit_entry)
        .await?;

    assert_eq!(updated.status, SubscriptionStatus::Canceled);
    let (action, target_id): (String, Option<Uuid>) =
        sqlx::query_as("SELECT action, target_id FROM audit.audit_log WHERE correlation_id = $1")
            .bind(correlation_id)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(action, "subscription.canceled");
    assert_eq!(target_id, Some(created.id.as_uuid()));
    Ok(())
}

#[tokio::test]
async fn cancel_still_audits_when_the_snapshot_is_stale() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created =
        create_subscription(&repo, tenant, customer_id, plan_id, "sub_cancel_stale").await?;
    let future = now_micros() + Duration::hours(1);
    let _ = repo
        .apply_event(
            tenant,
            created.id,
            SubscriptionStatus::Active,
            created.current_period_start,
            created.current_period_end,
            created.cancel_at_period_end,
            future,
            entry(tenant, created.id.as_uuid(), Action::SubscriptionUpdated),
        )
        .await?;
    let mut stale_snapshot = snapshot_of(&created);
    stale_snapshot.status = SubscriptionStatus::Canceled;
    let audit_entry = entry(tenant, created.id.as_uuid(), Action::SubscriptionCanceled);
    let correlation_id = audit_entry.correlation_id().as_uuid();

    let updated = repo
        .cancel(
            tenant,
            created.id,
            stale_snapshot,
            now_micros(),
            audit_entry,
        )
        .await?;

    assert_eq!(
        updated.status,
        SubscriptionStatus::Active,
        "the stale cancellation must not regress the status the newer event set"
    );
    let audit_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit.audit_log WHERE correlation_id = $1")
            .bind(correlation_id)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(
        audit_rows, 1,
        "Stripe already confirmed the cancellation, so the request is audited regardless"
    );
    Ok(())
}

#[tokio::test]
async fn cancel_rolls_back_when_the_audit_insert_fails() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created = create_subscription(
        &repo,
        tenant,
        customer_id,
        plan_id,
        "sub_cancel_audit_fails",
    )
    .await?;
    fail_audit_insert_for(&db.pool, created.id.as_uuid()).await?;
    let mut snapshot = snapshot_of(&created);
    snapshot.status = SubscriptionStatus::Canceled;

    let result = repo
        .cancel(
            tenant,
            created.id,
            snapshot,
            now_micros(),
            entry(tenant, created.id.as_uuid(), Action::SubscriptionCanceled),
        )
        .await;

    assert!(result.is_err());
    let after = repo
        .find(tenant, created.id)
        .await?
        .ok_or("row still exists")?;
    assert_eq!(
        after.status, created.status,
        "a failed audit insert must leave the snapshot apply rolled back"
    );
    Ok(())
}

#[tokio::test]
async fn cancel_writes_no_audit_row_when_the_business_write_fails() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let repo = PgSubscriptionRepository::new(db.pool.clone());
    let tenant = TenantId::new(Uuid::new_v4());
    let (customer_id, plan_id) = seed(&db.pool, tenant).await?;
    let created = create_subscription(
        &repo,
        tenant,
        customer_id,
        plan_id,
        "sub_cancel_business_fails",
    )
    .await?;
    fail_subscriptions_update_for(&db.pool, "sub_cancel_business_fails").await?;
    let mut snapshot = snapshot_of(&created);
    snapshot.status = SubscriptionStatus::Canceled;
    let audit_entry = entry(tenant, created.id.as_uuid(), Action::SubscriptionCanceled);
    let correlation_id = audit_entry.correlation_id().as_uuid();

    let result = repo
        .cancel(tenant, created.id, snapshot, now_micros(), audit_entry)
        .await;

    assert!(result.is_err());
    let audit_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit.audit_log WHERE correlation_id = $1")
            .bind(correlation_id)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(audit_rows, 0);
    Ok(())
}
