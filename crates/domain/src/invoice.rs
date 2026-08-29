use core::fmt;

use time::OffsetDateTime;
use uuid::Uuid;

use crate::{CustomerId, DomainError, Money, SubscriptionId, TenantId};

/// Identifies an `Invoice`. Distinct from other entities' ids so the compiler
/// rejects passing the wrong id where an invoice id is expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InvoiceId(Uuid);

impl InvoiceId {
    /// Wraps a raw `Uuid` as an `InvoiceId`.
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the underlying `Uuid`.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

/// The payment state of an `Invoice` — the subset `init-spec.md` §10.4 acts
/// on (`invoice.paid`, `invoice.payment_failed`) plus the initial state.
/// Stored as `TEXT`, mapped here rather than as a Postgres `ENUM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvoiceStatus {
    /// Issued, not yet paid.
    Open,
    /// Paid in full.
    Paid,
    /// A payment attempt failed.
    Failed,
}

impl InvoiceStatus {
    /// The wire/storage form.
    pub fn as_str(&self) -> &'static str {
        match self {
            InvoiceStatus::Open => "open",
            InvoiceStatus::Paid => "paid",
            InvoiceStatus::Failed => "failed",
        }
    }
}

impl fmt::Display for InvoiceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for InvoiceStatus {
    type Error = DomainError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "open" => Ok(InvoiceStatus::Open),
            "paid" => Ok(InvoiceStatus::Paid),
            "failed" => Ok(InvoiceStatus::Failed),
            other => Err(DomainError::Repository(format!(
                "unknown invoice status: {other:?}"
            ))),
        }
    }
}

/// A tenant's invoice, mirroring a Stripe invoice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoice {
    /// The invoice's id.
    pub id: InvoiceId,
    /// The tenant this invoice belongs to.
    pub tenant_id: TenantId,
    /// The customer being billed.
    pub customer_id: CustomerId,
    /// The subscription this invoice is for, if any. An invoice can exist
    /// against a customer before a subscription is linked (`init-spec.md`
    /// §10.4 bootstrap).
    pub subscription_id: Option<SubscriptionId>,
    /// The linked Stripe invoice id.
    pub stripe_invoice_id: String,
    /// The invoiced amount.
    pub amount: Money,
    /// The current payment state.
    pub status: InvoiceStatus,
    /// When the invoice was created.
    pub created_at: OffsetDateTime,
    /// When the invoice was soft-deleted, if at all.
    pub deleted_at: Option<OffsetDateTime>,
}

/// Port for persisting and querying `Invoice` records. Implemented by an
/// adapter crate (`persistence`); no I/O here.
#[allow(async_fn_in_trait)]
pub trait InvoiceRepository {
    /// Creates a new invoice for the given tenant.
    async fn create(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        subscription_id: Option<SubscriptionId>,
        stripe_invoice_id: String,
        amount: Money,
        status: InvoiceStatus,
    ) -> Result<Invoice, DomainError>;

    /// Finds an invoice by id, scoped to the tenant. Returns `None` if it
    /// does not exist or has been soft-deleted.
    async fn find(
        &self,
        tenant_id: TenantId,
        id: InvoiceId,
    ) -> Result<Option<Invoice>, DomainError>;

    /// Lists all invoices for the given tenant, excluding soft-deleted ones.
    async fn list(&self, tenant_id: TenantId) -> Result<Vec<Invoice>, DomainError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_as_uuid() {
        let id = Uuid::new_v4();
        let invoice_id = InvoiceId::new(id);
        assert_eq!(invoice_id.as_uuid(), id);
    }

    #[test]
    fn status_round_trips_through_str() {
        for status in [InvoiceStatus::Open, InvoiceStatus::Paid, InvoiceStatus::Failed] {
            assert_eq!(InvoiceStatus::try_from(status.as_str()), Ok(status));
        }
    }

    #[test]
    fn unknown_status_is_rejected() {
        assert!(InvoiceStatus::try_from("void").is_err());
    }
}
