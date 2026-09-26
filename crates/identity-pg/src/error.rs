use std::fmt::Display;

use identity_domain::RepositoryError;
use thiserror::Error;

/// Errors from the Postgres identity adapter's own setup, wrapping
/// `sqlx::Error`. Repository methods return `identity_domain`'s opaque
/// [`RepositoryError`] instead -- see `repository_error`.
#[derive(Debug, Error)]
pub enum IdentityPgError {
    /// A database error.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

/// Renders an adapter error into the domain's opaque repository error. The
/// message reaches server logs only; the identity API never shows it to a
/// caller.
pub(crate) fn repository_error(err: impl Display) -> RepositoryError {
    RepositoryError(err.to_string())
}
