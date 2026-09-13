//! Proves `PgAuditSink` actually writes, against a real Postgres -- the
//! part `service`'s own tests (`StubAuditSink`) cannot cover, since they
//! never touch a database at all.

mod common;

use std::error::Error;

use audit::{Action, Actor, AuditEntry, AuditSink, CorrelationId, Target, TargetId, TenantId};
use audit_pg::PgAuditSink;
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn record_writes_a_row_using_its_own_connection() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;
    let sink = PgAuditSink::new(db.pool.clone());
    let target_id = Uuid::new_v4();
    let correlation_id = Uuid::new_v4();
    let entry = AuditEntry::new(
        TenantId::new(Uuid::new_v4()),
        Actor::System,
        Action::SetupIntentCreated,
        Target::Customer(TargetId::new(target_id)),
        OffsetDateTime::now_utc(),
        CorrelationId::new(correlation_id),
    );

    sink.record(entry).await?;

    let (action, target_kind, found_target_id): (String, String, Option<Uuid>) = sqlx::query_as(
        "SELECT action, target_kind, target_id FROM audit.audit_log WHERE correlation_id = $1",
    )
    .bind(correlation_id)
    .fetch_one(&db.pool)
    .await?;
    assert_eq!(action, "setup_intent.created");
    assert_eq!(target_kind, "customer");
    assert_eq!(found_target_id, Some(target_id));
    Ok(())
}
