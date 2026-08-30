//! Domain models, value objects, ports and error taxonomy. No I/O.

mod billing_provider;
mod customer;
mod error;
mod invoice;
mod money;
mod outbound_request;
mod payment_method;
mod plan;
mod subscription;
mod tenant;
mod webhook_event;
mod webhook_verifier;

pub use billing_provider::{
    BillingProvider, CancellationTiming, CreateCustomerParams, CustomerSnapshot,
    SubscriptionSnapshot, UpdateCustomerParams,
};
pub use customer::{Customer, CustomerId, CustomerRepository};
pub use error::DomainError;
pub use invoice::{Invoice, InvoiceId, InvoiceRepository, InvoiceStatus};
pub use money::{Currency, Money};
pub use outbound_request::{OutboundRequest, OutboundRequestId, OutboundRequestRepository};
pub use payment_method::{PaymentMethod, PaymentMethodId, PaymentMethodRepository};
pub use plan::{Plan, PlanId, PlanRepository};
pub use subscription::{Subscription, SubscriptionId, SubscriptionRepository, SubscriptionStatus};
pub use tenant::TenantId;
pub use webhook_event::{WebhookEvent, WebhookEventId, WebhookEventRepository};
pub use webhook_verifier::{VerifiedEvent, WebhookReceipt, WebhookVerifier};
