use crate::error::DomainError;

/// A currency this system knows how to bill in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Currency {
    /// US Dollar.
    Usd,
    /// Euro.
    Eur,
    /// British Pound.
    Gbp,
}

/// A monetary amount, stored in the currency's smallest unit (e.g. cents).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Money {
    amount_minor: i64,
    currency: Currency,
}

impl Money {
    /// Constructs a monetary amount from a minor-unit integer and currency.
    pub fn new(amount_minor: i64, currency: Currency) -> Self {
        Self {
            amount_minor,
            currency,
        }
    }

    /// The amount in the currency's smallest unit.
    pub fn amount_minor(&self) -> i64 {
        self.amount_minor
    }

    /// The currency of this amount.
    pub fn currency(&self) -> Currency {
        self.currency
    }

    /// Adds two amounts of the same currency, checking for overflow.
    ///
    /// Returns `Err(DomainError::CurrencyMismatch)` if the currencies differ,
    /// or `Err(DomainError::AmountOverflow)` if the sum overflows `i64`.
    pub fn add(&self, other: &Money) -> Result<Money, DomainError> {
        if self.currency != other.currency {
            return Err(DomainError::CurrencyMismatch);
        }
        let amount_minor = self
            .amount_minor
            .checked_add(other.amount_minor)
            .ok_or(DomainError::AmountOverflow)?;
        Ok(Money::new(amount_minor, self.currency))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_sums_matching_currencies() {
        let a = Money::new(100, Currency::Usd);
        let b = Money::new(250, Currency::Usd);
        assert_eq!(a.add(&b), Ok(Money::new(350, Currency::Usd)));
    }

    #[test]
    fn add_rejects_mismatched_currencies() {
        let a = Money::new(100, Currency::Usd);
        let b = Money::new(100, Currency::Eur);
        assert_eq!(a.add(&b), Err(DomainError::CurrencyMismatch));
    }

    #[test]
    fn add_rejects_overflow() {
        let a = Money::new(i64::MAX, Currency::Usd);
        let b = Money::new(1, Currency::Usd);
        assert_eq!(a.add(&b), Err(DomainError::AmountOverflow));
    }
}
