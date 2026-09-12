use std::future::Future;

use audit::AuditEntry;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{CustomerId, DomainError, TenantId};

/// Identifies a `PaymentMethod`. Distinct from other entities' ids so the
/// compiler rejects passing the wrong id where a payment method id is expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaymentMethodId(Uuid);

impl PaymentMethodId {
    /// Wraps a raw `Uuid` as a `PaymentMethodId`.
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the underlying `Uuid`.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

/// A stored card for a customer, mirroring a Stripe payment method. `brand`
/// and `last4` are display metadata only; no card data lives here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentMethod {
    /// The payment method's id.
    pub id: PaymentMethodId,
    /// The tenant this payment method belongs to.
    pub tenant_id: TenantId,
    /// The customer this payment method is for.
    pub customer_id: CustomerId,
    /// The linked Stripe payment method id.
    pub stripe_payment_method_id: String,
    /// The card brand (e.g. `visa`).
    pub brand: String,
    /// The last four digits of the card.
    pub last4: String,
    /// Whether this is the customer's default payment method. Enforcing "at
    /// most one default per customer" is a `service`-layer concern, not a
    /// schema constraint here.
    pub is_default: bool,
    /// The `created` timestamp of the last webhook event applied to this row
    /// -- the ordering anchor that stops an older event overwriting a newer
    /// one. `None` means no event has
    /// been applied yet -- true of a row created outside the webhook path.
    pub last_event_created_at: Option<OffsetDateTime>,
    /// When the payment method was created.
    pub created_at: OffsetDateTime,
    /// When the payment method was soft-deleted, if at all.
    pub deleted_at: Option<OffsetDateTime>,
}

/// Port for persisting and querying `PaymentMethod` records. Implemented by an
/// adapter crate (`persistence`); no I/O here.
///
/// Written as `fn … -> impl Future<Output = …> + Send` rather than bare
/// `async fn`, matching `SubscriptionRepository` and `InvoiceRepository`: the
/// webhook path's `WebhookProcessor` goes behind `#[async_trait]` to
/// implement the object-safe `WebhookHandler` port, which boxes its futures
/// as `Send`, so every future it awaits must be `Send` too. Implementors may
/// still write `async fn` in the `impl` block; the bound is checked there.
pub trait PaymentMethodRepository {
    /// Creates a new payment method for the given tenant.
    fn create(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        stripe_payment_method_id: String,
        brand: String,
        last4: String,
        is_default: bool,
    ) -> impl Future<Output = Result<PaymentMethod, DomainError>> + Send;

    /// Finds a payment method by id, scoped to the tenant. Returns `None` if
    /// it does not exist or has been soft-deleted.
    fn find(
        &self,
        tenant_id: TenantId,
        id: PaymentMethodId,
    ) -> impl Future<Output = Result<Option<PaymentMethod>, DomainError>> + Send;

    /// Lists all payment methods for the given tenant, excluding soft-deleted ones.
    fn list(
        &self,
        tenant_id: TenantId,
    ) -> impl Future<Output = Result<Vec<PaymentMethod>, DomainError>> + Send;

    /// Finds a payment method by its Stripe id, scoped to the tenant. `None`
    /// if it does not exist, is soft-deleted, or belongs to another tenant.
    fn find_by_stripe_payment_method_id(
        &self,
        tenant_id: TenantId,
        stripe_payment_method_id: &str,
    ) -> impl Future<Output = Result<Option<PaymentMethod>, DomainError>> + Send;

    /// Mirrors a `payment_method.attached` (or equivalent) event, guarded by
    /// the ordering rule. An **upsert**, like
    /// [`InvoiceRepository::apply_event`](crate::InvoiceRepository::apply_event):
    /// `attached` is the first the module sees, so "mirror" means
    /// create-if-absent. One `INSERT … ON CONFLICT (tenant_id,
    /// stripe_payment_method_id) DO UPDATE … WHERE <ordering predicate>`;
    /// `rows_affected() == 0` is the stale signal
    /// ([`EventApplication::Stale`](crate::EventApplication), row unchanged).
    ///
    /// Takes `entry` directly -- this method has exactly one caller, the
    /// webhook path -- and writes it regardless of Applied vs. Stale, the
    /// same reasoning [`detach_event`](Self::detach_event) documents.
    #[allow(clippy::too_many_arguments)]
    fn apply_event(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        stripe_payment_method_id: &str,
        brand: &str,
        last4: &str,
        is_default: bool,
        event_created_at: OffsetDateTime,
        entry: AuditEntry,
    ) -> impl Future<Output = Result<crate::EventApplication, DomainError>> + Send;

    /// Applies a `payment_method.detached` event: a **soft** removal
    /// (`deleted_at = now()`), never a physical delete, guarded by the same
    /// ordering rule. **Precondition:** the row exists and is not
    /// soft-deleted -- callers `find_by_stripe_payment_method_id` first, so a
    /// zero-rows result is read as "a newer event already applied", the same
    /// contract as `SubscriptionRepository::apply_event`.
    ///
    /// Takes `entry` directly, like [`apply_event`](Self::apply_event):
    /// this method now has exactly one caller (the webhook path --
    /// `Writes::remove_payment_method` moved to
    /// [`remove`](Self::remove) in an earlier phase), so there is no second
    /// caller an `AuditEntry` parameter could starve of one. Written
    /// **regardless of whether this detach is stale**: Stripe reported a
    /// real event either way, and a rejected write is itself evidence worth
    /// keeping.
    fn detach_event(
        &self,
        tenant_id: TenantId,
        stripe_payment_method_id: &str,
        event_created_at: OffsetDateTime,
        entry: AuditEntry,
    ) -> impl Future<Output = Result<crate::EventApplication, DomainError>> + Send;

    /// The same soft-delete [`detach_event`](Self::detach_event) performs,
    /// in one transaction with the audit entry that records it. Used by
    /// `Writes::remove_payment_method` -- the caller-initiated removal.
    ///
    /// A separate method from `detach_event` rather than reusing it
    /// directly: this call's `AuditEntry` describes an explicit caller
    /// action, `detach_event`'s describes a webhook confirming one Stripe
    /// already knows about -- two different `Actor`/correlation-id
    /// sources that arrive through two different call paths (`Writes`
    /// here, `WebhookHandler` there), so keeping the entry points separate
    /// is what keeps neither path able to construct the other's kind of
    /// entry by accident.
    ///
    /// The entry is written **regardless of whether this detach is
    /// stale**: by the time this method runs, the caller has already asked
    /// Stripe to detach the card and Stripe has confirmed it, so the
    /// request happened and is worth recording even if a
    /// `payment_method.detached` webhook already soft-deleted the local row
    /// first -- see `detach_event`'s own docs on why that race is not an
    /// error.
    fn remove(
        &self,
        tenant_id: TenantId,
        stripe_payment_method_id: &str,
        event_created_at: OffsetDateTime,
        entry: AuditEntry,
    ) -> impl Future<Output = Result<crate::EventApplication, DomainError>> + Send;

    /// Makes `id` the customer's sole default in **one statement**: sets
    /// `is_default = (id = $target)` across the customer's non-deleted rows,
    /// so the old default is cleared and the new one set together -- never a
    /// window with two defaults or none.
    ///
    /// **Deliberately not guarded by the webhook ordering rule, and separate
    /// from [`apply_event`](Self::apply_event)** -- the same split as
    /// [`SubscriptionRepository::set_plan`](crate::SubscriptionRepository::set_plan).
    /// `apply_event` writes what a webhook reported and must be ordered
    /// against other webhooks; `is_default` chosen through this method is
    /// what *a caller explicitly asked for* via
    /// `POST /payment-methods/{id}/default`, and no webhook writes it that
    /// way, so there is no event for it to lose a race against. The caller
    /// runs this **after** Stripe's own
    /// `invoice_settings.default_payment_method` update has succeeded.
    ///
    /// **Precondition:** `id` names a live row for `(tenant_id, customer_id)`
    /// -- the caller looked it up first. A non-matching `id` clears every
    /// default for that customer and sets none, which is why the caller
    /// checks ownership before calling.
    ///
    /// Takes `entry` directly, unlike [`remove`](Self::remove)'s separate
    /// method: this write has exactly one caller
    /// (`Writes::set_default_payment_method`), no webhook ever reaches it,
    /// so there is no second caller an `AuditEntry` parameter could starve
    /// of one. Runs in one transaction with the audit insert (D1(h)).
    fn set_default(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        id: PaymentMethodId,
        entry: AuditEntry,
    ) -> impl Future<Output = Result<(), DomainError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_as_uuid() {
        let id = Uuid::new_v4();
        let payment_method_id = PaymentMethodId::new(id);
        assert_eq!(payment_method_id.as_uuid(), id);
    }
}
