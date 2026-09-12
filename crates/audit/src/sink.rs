use std::future::Future;

use thiserror::Error;

use crate::AuditEntry;

/// Port for writing one audit entry.
///
/// Exactly one method, and it is the only one this trait will ever have:
/// there is no `update` and no `delete`, not even a soft one. Append-only
/// is a property of this trait's shape, not a convention an implementor is
/// trusted to honour -- an adapter that wanted to overwrite history would
/// have to add a method here first, in a crate every consuming module
/// depends on, which is a far louder place to do it than a stray `UPDATE`
/// buried in one adapter's SQL.
///
/// Written as `fn … -> impl Future<Output = …> + Send` rather than a bare
/// `async fn`, the same reasoning `domain`'s repository ports use in the
/// module this crate is written alongside: a caller that erases this
/// behind a `dyn`-safe façade needs the `Send` bound stated on the trait
/// itself, not left to whichever type happens to implement it.
///
/// An implementation that must be atomic with a business write does not
/// reach for this trait at all -- it takes a caller-supplied connection or
/// transaction handle directly, so the transaction never has to be
/// expressed as a trait object. This port is for the case with no existing
/// unit of work to join: a caller that only needs to write one row.
pub trait AuditSink {
    /// Writes `entry`.
    fn record(&self, entry: AuditEntry) -> impl Future<Output = Result<(), AuditError>> + Send;
}

/// Errors from writing an audit entry.
///
/// Opaque by design, the same reasoning as `domain::DomainError::Repository`
/// in the module this crate is written alongside: this crate must not
/// depend on `sqlx` or any adapter's error type, so an adapter maps its own
/// error into this variant's message.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AuditError {
    /// The write failed. The message is opaque -- see the type's docs.
    #[error("audit write failed: {0}")]
    Write(String),
}
