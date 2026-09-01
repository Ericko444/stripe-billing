use std::future::Future;

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
    /// most one default per customer" is a `service`-layer concern
    /// (`init-spec.md` §7.4), not a schema constraint here.
    pub is_default: bool,
    /// The `created` timestamp of the last webhook event applied to this row
    /// (`init-spec.md` §10.2's ordering anchor). `None` means no event has
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
    /// `init-spec.md` §10.2's ordering rule. An **upsert**, like
    /// [`InvoiceRepository::apply_event`](crate::InvoiceRepository::apply_event):
    /// `attached` is the first the module sees, so "mirror" means
    /// create-if-absent. One `INSERT … ON CONFLICT (tenant_id,
    /// stripe_payment_method_id) DO UPDATE … WHERE <ordering predicate>`;
    /// `rows_affected() == 0` is the stale signal
    /// ([`EventApplication::Stale`](crate::EventApplication), row unchanged).
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
    ) -> impl Future<Output = Result<crate::EventApplication, DomainError>> + Send;

    /// Applies a `payment_method.detached` event: a **soft** removal
    /// (`deleted_at = now()`), never a physical delete, guarded by the same
    /// ordering rule. **Precondition:** the row exists and is not
    /// soft-deleted -- callers `find_by_stripe_payment_method_id` first, so a
    /// zero-rows result is read as "a newer event already applied", the same
    /// contract as `SubscriptionRepository::apply_event`.
    fn detach_event(
        &self,
        tenant_id: TenantId,
        stripe_payment_method_id: &str,
        event_created_at: OffsetDateTime,
    ) -> impl Future<Output = Result<crate::EventApplication, DomainError>> + Send;
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
