//! sqlx adapters implementing the domain repository ports.

mod currency_codec;
mod customer_repository;
mod error;
mod invoice_repository;
mod outbound_request_repository;
mod payment_method_repository;
mod plan_repository;
mod subscription_repository;
mod webhook_event_repository;

pub use customer_repository::PgCustomerRepository;
pub use error::RepositoryError;
pub use invoice_repository::PgInvoiceRepository;
pub use outbound_request_repository::PgOutboundRequestRepository;
pub use payment_method_repository::PgPaymentMethodRepository;
pub use plan_repository::PgPlanRepository;
pub use subscription_repository::PgSubscriptionRepository;
pub use webhook_event_repository::PgWebhookEventRepository;

use sqlx::PgPool;

/// Applies this crate's `migrations/` directory to `pool`.
///
/// `sqlx::migrate!` embeds the SQL in the binary at compile time. It is not
/// read from disk when this runs, which is what makes the function work
/// outside a checkout of this repository: a crate installed from a registry
/// has no workspace root to look two directories up into, and a binary built
/// on CI and copied to a container has no migration files beside it at all.
/// The ban this project observes is on the compile-time *query* macros
/// (`query!`, `query_as!`), which need a live database; `migrate!` needs
/// only the directory, and no `DATABASE_URL`.
///
/// Safe to call repeatedly: already-applied migrations are skipped.
pub async fn run_migrations(pool: &PgPool) -> Result<(), RepositoryError> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(sqlx::Error::from)?;
    Ok(())
}
