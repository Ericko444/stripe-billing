//! Postgres adapter for the identity module: the `identity` schema, its
//! migrator, and the repositories implementing `identity-domain`'s ports.
//!
//! Runs against the same database as the billing module and the audit
//! journal. Each of the three owns a schema and tracks its migrations in
//! its own table, so none of them has to know the others exist -- the
//! pattern `audit-pg` established.
//!
//! Every write that has an audit entry writes it with `audit_pg::insert` on
//! the same transaction, so the business change and the record of it commit
//! or fail together.

mod error;
mod fan_out;
mod membership_repository;
mod password_token_repository;
mod session_repository;
mod user_repository;

pub use error::IdentityPgError;
pub use membership_repository::PgMembershipRepository;
pub use password_token_repository::PgPasswordTokenRepository;
pub use session_repository::PgSessionRepository;
pub use user_repository::PgUserRepository;

use sqlx::PgPool;
use sqlx::migrate::Migrator;

/// Applies this crate's `migrations/` directory to `pool`, tracking applied
/// migrations in `identity._sqlx_migrations`.
///
/// The billing module's migrator uses the default `_sqlx_migrations` and
/// `audit-pg`'s uses `audit._sqlx_migrations`. A third migrator sharing
/// either table would collide with it on version numbers; schema- and
/// table-qualifying this one is what lets all three run against one
/// database, in any order, repeatedly.
pub async fn run_migrations(pool: &PgPool) -> Result<(), IdentityPgError> {
    let migrations_dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"));
    let mut migrator = Migrator::new(migrations_dir)
        .await
        .map_err(sqlx::Error::from)?;
    migrator.create_schema("identity");
    migrator.dangerous_set_table_name("identity._sqlx_migrations");
    migrator.run(pool).await.map_err(sqlx::Error::from)?;
    Ok(())
}
