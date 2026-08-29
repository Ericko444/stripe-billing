//! Maps the domain `Currency` to and from the `CHAR(3)` column used by
//! every money-bearing table. Shared by the `Plan` and `Invoice` adapters.

use domain::{Currency, DomainError};

/// Parses a `CHAR(3)` currency code into the domain `Currency`. An
/// unrecognized code is a repository-level error, not a panic.
pub(crate) fn currency_from_code(code: &str) -> Result<Currency, DomainError> {
    match code {
        "USD" => Ok(Currency::Usd),
        "EUR" => Ok(Currency::Eur),
        "GBP" => Ok(Currency::Gbp),
        other => Err(DomainError::Repository(format!(
            "unknown currency code: {other:?}"
        ))),
    }
}

/// Renders a domain `Currency` as its `CHAR(3)` code for storage.
pub(crate) fn currency_code(currency: Currency) -> &'static str {
    match currency {
        Currency::Usd => "USD",
        Currency::Eur => "EUR",
        Currency::Gbp => "GBP",
    }
}
