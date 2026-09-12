//! Postgres adapter for `audit`: its own schema, migrator and append-only
//! grant.
//!
//! Owns `audit.audit_log` and its migration -- see `migrations/` for the
//! schema, the append-only grant, and why append-only is enforced at the
//! grant layer rather than trusted to the application.
//!
//! No `insert` function ships yet, deliberately: `audit::Action` and
//! `audit::Target` are still empty enums (each vertical slice adds its own
//! variant as it lands), which makes an `AuditEntry` impossible to
//! construct today. Writing an `insert` now would mean matching those
//! enums exhaustively with zero arms, which makes everything after that
//! match unreachable and trips the workspace's `-D warnings` gate for no
//! real reason -- there is nothing yet to insert. It is added once the
//! first variant lands.

mod error;

pub use error::AuditPgError;

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
