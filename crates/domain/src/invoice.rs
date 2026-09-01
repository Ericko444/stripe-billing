use core::fmt;
use std::future::Future;

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
    /// The `created` timestamp of the last webhook event applied to this row
    /// (`init-spec.md` §10.2's ordering anchor). `None` means no event has
    /// been applied yet -- true of a row created outside the webhook path,
    /// and momentarily of the first mirror write before it commits.
    pub last_event_created_at: Option<OffsetDateTime>,
    /// When the invoice was created.
    pub created_at: OffsetDateTime,
    /// When the invoice was soft-deleted, if at all.
    pub deleted_at: Option<OffsetDateTime>,
}

/// An opaque position in a tenant's invoice list, ordered by
/// `(created_at DESC, id DESC)`. Carries only what the keyset query needs to
/// resume from: the sort key of the last row of the previous page.
///
/// **Opaque on the wire.** `api` base64-encodes it, so this shape can change
/// without a wire break. It is neither encrypted nor authenticated, and does
/// not need to be: it names a position *within the caller's own tenant*, and
/// [`InvoiceRepository::list_page`] is tenant-scoped regardless of what the
/// cursor says. A forged cursor can only reposition a caller inside data they
/// already have.
///
/// No public field: constructed by [`InvoiceRepository::list_page`] for a
/// page's `next`, or by `api`'s codec via [`InvoiceCursor::new`] when
/// decoding one off the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvoiceCursor {
    created_at: OffsetDateTime,
    id: InvoiceId,
}

impl InvoiceCursor {
    /// Builds a cursor from a row's sort key.
    pub fn new(created_at: OffsetDateTime, id: InvoiceId) -> Self {
        Self { created_at, id }
    }

    /// The `created_at` half of the keyset position.
    pub fn created_at(&self) -> OffsetDateTime {
        self.created_at
    }

    /// The `id` half of the keyset position -- the tiebreaker when two rows
    /// share a `created_at`.
    pub fn id(&self) -> InvoiceId {
        self.id
    }
}

/// One page of a tenant's invoices, newest first, and where to resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoicePage {
    /// The page's invoices, ordered `(created_at DESC, id DESC)`.
    pub items: Vec<Invoice>,
    /// The cursor to pass as `after` for the next page, or `None` when this
    /// is the last page. Never a cursor that would yield an empty page --
    /// [`InvoiceRepository::list_page`] looks one row past the page to decide.
    pub next: Option<InvoiceCursor>,
}

/// Port for persisting and querying `Invoice` records. Implemented by an
/// adapter crate (`persistence`); no I/O here.
///
/// Written as `fn … -> impl Future<Output = …> + Send` rather than bare
/// `async fn`, matching `SubscriptionRepository` and
/// `WebhookEventRepository`: the webhook path's `WebhookProcessor` goes
/// behind `#[async_trait]` to implement the object-safe `WebhookHandler`
/// port, which boxes its futures as `Send`, so every future it awaits --
/// including these -- must be `Send` too. A bare `async fn` in a trait does
/// not promise that for a generic implementor. Implementors may still write
/// `async fn` in the `impl` block; the bound is checked there.
pub trait InvoiceRepository {
    /// Creates a new invoice for the given tenant.
    fn create(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        subscription_id: Option<SubscriptionId>,
        stripe_invoice_id: String,
        amount: Money,
        status: InvoiceStatus,
    ) -> impl Future<Output = Result<Invoice, DomainError>> + Send;

    /// Finds an invoice by id, scoped to the tenant. Returns `None` if it
    /// does not exist or has been soft-deleted.
    fn find(
        &self,
        tenant_id: TenantId,
        id: InvoiceId,
    ) -> impl Future<Output = Result<Option<Invoice>, DomainError>> + Send;

    /// Lists all invoices for the given tenant, excluding soft-deleted ones.
    ///
    /// Kept alongside [`list_page`](Self::list_page): still the simplest call
    /// for tests and for callers that genuinely want every row.
    fn list(
        &self,
        tenant_id: TenantId,
    ) -> impl Future<Output = Result<Vec<Invoice>, DomainError>> + Send;

    /// One keyset-paginated page of the tenant's invoices, newest first
    /// (`created_at DESC, id DESC`), soft-deleted rows excluded.
    ///
    /// `after` is the previous page's [`InvoicePage::next`]; `None` starts at
    /// the newest row. `limit` is the maximum number of rows in the returned
    /// page -- the implementation may read one more internally to decide
    /// whether `next` should be set.
    ///
    /// **Keyset, not `OFFSET`.** The seek is a row-value comparison
    /// `(created_at, id) < (after.created_at, after.id)`, which is an index
    /// range scan regardless of how deep the caller has paged, and is stable
    /// under concurrent inserts: it names a position in the ordering, not a
    /// count of rows to skip. `(created_at, id)` rather than `created_at`
    /// alone because `created_at` is not unique -- ties would drop or
    /// duplicate rows across a page boundary.
    fn list_page(
        &self,
        tenant_id: TenantId,
        after: Option<InvoiceCursor>,
        limit: u16,
    ) -> impl Future<Output = Result<InvoicePage, DomainError>> + Send;

    /// Finds an invoice by its Stripe id, scoped to the tenant. Returns
    /// `None` if it does not exist, has been soft-deleted, or belongs to a
    /// different tenant. Tenant-scoped, like
    /// [`SubscriptionRepository::find_by_stripe_subscription_id`](crate::SubscriptionRepository::find_by_stripe_subscription_id):
    /// the webhook path resolves the tenant from the invoice's customer
    /// before it needs this lookup.
    fn find_by_stripe_invoice_id(
        &self,
        tenant_id: TenantId,
        stripe_invoice_id: &str,
    ) -> impl Future<Output = Result<Option<Invoice>, DomainError>> + Send;

    /// Mirrors a webhook event's invoice state, guarded by `init-spec.md`
    /// §10.2's ordering rule: admitted when `event_created_at` is `>=` the
    /// row's current `last_event_created_at` (or that column is `NULL`),
    /// rejected -- [`EventApplication::Stale`](crate::EventApplication), row
    /// unchanged -- otherwise.
    ///
    /// **An upsert, unlike
    /// [`SubscriptionRepository::apply_event`](crate::SubscriptionRepository::apply_event).**
    /// There is no `invoice.created` webhook (§10.4); `invoice.paid` /
    /// `invoice.payment_failed` are the first the module sees, and "mirror
    /// invoice" means creating the local row if it is absent. So this takes
    /// the full set of insertable columns rather than a local `InvoiceId`,
    /// and keys on `stripe_invoice_id`.
    ///
    /// **The guard is in the statement, not a read-then-compare in Rust.**
    /// The write is one `INSERT … ON CONFLICT (tenant_id, stripe_invoice_id)
    /// DO UPDATE … WHERE <ordering predicate>`. A fresh insert or an admitted
    /// update reports one row affected; a conflict whose predicate fails
    /// reports zero, and `rows_affected() == 0` *is* the stale signal --
    /// resolved atomically by Postgres, not concluded here.
    #[allow(clippy::too_many_arguments)]
    fn apply_event(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        subscription_id: Option<SubscriptionId>,
        stripe_invoice_id: &str,
        amount: Money,
        status: InvoiceStatus,
        event_created_at: OffsetDateTime,
    ) -> impl Future<Output = Result<crate::EventApplication, DomainError>> + Send;
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
        for status in [
            InvoiceStatus::Open,
            InvoiceStatus::Paid,
            InvoiceStatus::Failed,
        ] {
            assert_eq!(InvoiceStatus::try_from(status.as_str()), Ok(status));
        }
    }

    #[test]
    fn unknown_status_is_rejected() {
        assert!(InvoiceStatus::try_from("void").is_err());
    }
}
