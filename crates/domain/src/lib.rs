//! Domain models, value objects, ports and error taxonomy. No I/O.

mod customer;
mod error;
mod invoice;
mod money;
mod payment_method;
mod plan;
mod subscription;
mod tenant;

pub use customer::{Customer, CustomerId, CustomerRepository};
pub use error::DomainError;
pub use invoice::{Invoice, InvoiceId, InvoiceRepository, InvoiceStatus};
pub use money::{Currency, Money};
pub use payment_method::{PaymentMethod, PaymentMethodId, PaymentMethodRepository};
pub use plan::{Plan, PlanId, PlanRepository};
pub use subscription::{Subscription, SubscriptionId, SubscriptionRepository, SubscriptionStatus};
pub use tenant::TenantId;
