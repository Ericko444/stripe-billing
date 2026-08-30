use thiserror::Error;

/// Errors from the persistence layer, wrapping `sqlx::Error`.
#[derive(Debug, Error)]
pub enum RepositoryError {
    /// A unique constraint was violated (Postgres SQLSTATE `23505`).
    #[error("unique constraint violated")]
    UniqueViolation,

    /// A database error.
    #[error(transparent)]
    Other(#[from] sqlx::Error),
}

impl RepositoryError {
    /// Classifies a `sqlx::Error` as `UniqueViolation` when it carries
    /// Postgres SQLSTATE `23505`, or `Other` otherwise.
    ///
    /// Matches on the SQLSTATE rather than the error message: Postgres is
    /// free to reword the message, but the code is a stable part of the
    /// wire protocol.
    ///
    /// Not the blanket `From<sqlx::Error>` impl (`Other`'s `#[from]`) —
    /// deliberately. Making unique-violation detection global would change
    /// what every existing repository's write path returns, and several
    /// already have passing tests asserting the current
    /// `DomainError::Repository(_)` shape on their own unique violations.
    /// Callers that want this distinction opt in explicitly by calling
    /// `classify` instead of relying on `?`/`From`.
    pub fn classify(err: sqlx::Error) -> Self {
        let is_unique_violation = err
            .as_database_error()
            .and_then(|db_err| db_err.code())
            .as_deref()
            == Some("23505");
        if is_unique_violation {
            RepositoryError::UniqueViolation
        } else {
            RepositoryError::Other(err)
        }
    }
}
