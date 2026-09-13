use thiserror::Error;

/// Errors from the Postgres identity adapter, wrapping `sqlx::Error`.
#[derive(Debug, Error)]
pub enum IdentityPgError {
    /// A database error.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}
