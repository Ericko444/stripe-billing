//! sqlx adapters implementing the domain repository ports.

mod customer_repository;
mod error;
mod plan_repository;
mod subscription_repository;

pub use customer_repository::PgCustomerRepository;
pub use error::RepositoryError;
pub use plan_repository::PgPlanRepository;
pub use subscription_repository::PgSubscriptionRepository;

use sqlx::PgPool;
use sqlx::migrate::Migrator;

/// Applies the workspace's `migrations/` directory to `pool` at runtime.
/// Safe to call repeatedly: already-applied migrations are skipped.
pub async fn run_migrations(pool: &PgPool) -> Result<(), RepositoryError> {
    let migrations_dir =
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations"));
    let migrator = Migrator::new(migrations_dir)
        .await
        .map_err(sqlx::Error::from)?;
    migrator.run(pool).await.map_err(sqlx::Error::from)?;
    Ok(())
}
