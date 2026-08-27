use thiserror::Error;

/// Errors from the persistence layer, wrapping `sqlx::Error`.
#[derive(Debug, Error)]
pub enum RepositoryError {
    /// A database error.
    #[error(transparent)]
    Other(#[from] sqlx::Error),
}
