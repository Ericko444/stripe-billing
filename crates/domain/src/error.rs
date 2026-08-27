use thiserror::Error;

/// Errors representing violations of domain invariants.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    /// Attempted to combine monetary amounts in two different currencies.
    #[error("currency mismatch")]
    CurrencyMismatch,

    /// A monetary arithmetic operation overflowed.
    #[error("amount overflow")]
    AmountOverflow,

    /// The requested entity does not exist.
    #[error("not found")]
    NotFound,
}
