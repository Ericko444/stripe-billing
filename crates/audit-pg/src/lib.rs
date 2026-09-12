//! Postgres adapter for `audit`: its own schema, migrator, append-only
//! grant, and [`insert`].
//!
//! Owns `audit.audit_log` and its migration -- see `migrations/` for the
//! schema, the append-only grant, and why append-only is enforced at the
//! grant layer rather than trusted to the application.
//!
//! No `AuditSink` implementation ships here yet: [`insert`] is the
//! primitive a caller composes into its own transaction (`persistence`
//! does exactly this for the audited write paths), and a pool-based
//! `AuditSink` for callers with no existing transaction to join is added
//! once something actually needs it (the two Stripe-only write routes,
//! Task 10).

mod error;
mod insert;

pub use error::AuditPgError;
pub use insert::insert;

use sqlx::PgPool;
use sqlx::migrate::Migrator;

/// Applies this crate's `migrations/` directory to `pool`, tracking applied
/// migrations in `audit._sqlx_migrations` rather than the default
/// `_sqlx_migrations`.
///
/// The billing module's own `persistence::run_migrations` runs against the
/// same database and tracks its state in the default table name -- the two
/// migrators would collide on version numbers if both used it. Schema- and
/// table-qualifying this one is what makes running both against one
/// database safe, and is the reason `audit` and the billing module can
/// share a database at all without either coordinating with the other.
///
/// Safe to call repeatedly: already-applied migrations are skipped.
pub async fn run_migrations(pool: &PgPool) -> Result<(), AuditPgError> {
    let migrations_dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"));
    let mut migrator = Migrator::new(migrations_dir)
        .await
        .map_err(sqlx::Error::from)?;
    migrator.create_schema("audit");
    migrator.dangerous_set_table_name("audit._sqlx_migrations");
    migrator.run(pool).await.map_err(sqlx::Error::from)?;
    Ok(())
}
