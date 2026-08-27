//! Domain models, value objects, ports and error taxonomy. No I/O.

mod error;
mod money;
mod tenant;

pub use error::DomainError;
pub use money::{Currency, Money};
pub use tenant::TenantId;
