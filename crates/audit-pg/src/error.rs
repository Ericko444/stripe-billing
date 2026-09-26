use thiserror::Error;

/// Errors from the Postgres audit adapter, wrapping `sqlx::Error`.
#[derive(Debug, Error)]
pub enum AuditPgError {
    /// A database error.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}
