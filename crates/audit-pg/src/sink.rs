use audit::{AuditEntry, AuditError, AuditSink};
use sqlx::PgPool;

use crate::insert;

/// Pool-based [`AuditSink`], for a caller with no existing transaction to
/// join.
///
/// Every audited write elsewhere in this module rides inside the
/// transaction of the business write it describes (`persistence`'s own
/// audited repository methods call [`insert`](crate::insert) directly on
/// that transaction's connection). The two Stripe-only write routes --
/// `create_setup_intent`, `start_checkout_session` -- have no local write
/// at all to join: nothing changes in this database until a later webhook
/// confirms it. `PgAuditSink` is what they use instead, acquiring its own
/// connection from the pool per call.
pub struct PgAuditSink {
    pool: PgPool,
}

impl PgAuditSink {
    /// Wraps a `PgPool` as a `PgAuditSink`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl AuditSink for PgAuditSink {
    async fn record(&self, entry: AuditEntry) -> Result<(), AuditError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|err| AuditError::Write(err.to_string()))?;
        insert(&mut conn, &entry)
            .await
            .map_err(|err| AuditError::Write(err.to_string()))
    }
}
