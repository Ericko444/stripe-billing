use core::fmt;
use std::future::Future;

use time::OffsetDateTime;
use uuid::Uuid;

use crate::{CustomerId, DomainError, PlanId, TenantId};

/// Identifies a `Subscription`. Distinct from other entities' ids so the
/// compiler rejects passing the wrong id where a subscription id is expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(Uuid);

impl SubscriptionId {
    /// Wraps a raw `Uuid` as a `SubscriptionId`.
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the underlying `Uuid`.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

/// The lifecycle state of a `Subscription` — the subset of Stripe
/// subscription statuses this system acts on. Stored
/// as `TEXT`, mapped here rather than as a Postgres `ENUM` so a new Stripe
/// status is a match arm, not an `ALTER TYPE` migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionStatus {
    /// Current and paid.
    Active,
    /// A payment failed and Stripe is retrying.
    PastDue,
    /// Ended, whether by the tenant or after failed retries.
    Canceled,
    /// Created but the first invoice hasn't been paid yet — Stripe's status
    /// immediately after `create_subscription`.
    Incomplete,
    /// The first invoice was never paid within Stripe's window (23 hours by
    /// default) and Stripe closed the subscription on its own. Terminal, the
    /// same as `Canceled` — the tenant is never billed and nothing here can
    /// revive it. A cancel issued against an already-`Incomplete`
    /// subscription lands here rather than `Canceled` — Stripe's own
    /// distinction between "never collected" and "was active, then ended".
    IncompleteExpired,
}

impl SubscriptionStatus {
    /// The wire/storage form, matching Stripe's own strings.
    pub fn as_str(&self) -> &'static str {
        match self {
            SubscriptionStatus::Active => "active",
            SubscriptionStatus::PastDue => "past_due",
            SubscriptionStatus::Canceled => "canceled",
            SubscriptionStatus::Incomplete => "incomplete",
            SubscriptionStatus::IncompleteExpired => "incomplete_expired",
        }
    }
}

impl fmt::Display for SubscriptionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for SubscriptionStatus {
    type Error = DomainError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "active" => Ok(SubscriptionStatus::Active),
            "past_due" => Ok(SubscriptionStatus::PastDue),
            "canceled" => Ok(SubscriptionStatus::Canceled),
            "incomplete" => Ok(SubscriptionStatus::Incomplete),
            "incomplete_expired" => Ok(SubscriptionStatus::IncompleteExpired),
            other => Err(DomainError::Repository(format!(
                "unknown subscription status: {other:?}"
            ))),
        }
    }
}

/// Whether [`SubscriptionRepository::apply_event`]'s ordering guard admitted
/// the write. A named two-state value rather than a
/// bare `bool`, so a call site cannot silently invert it -- the same
/// reasoning behind `CancellationTiming`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum EventApplication {
    /// The row was updated and `last_event_created_at` advanced.
    Applied,
    /// The guard rejected the write: a newer event has already been applied
    /// to this row. The row is unchanged.
    Stale,
}

/// A tenant's subscription to a `Plan`, mirroring a Stripe subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    /// The subscription's id.
    pub id: SubscriptionId,
    /// The tenant this subscription belongs to.
    pub tenant_id: TenantId,
    /// The customer being billed.
    pub customer_id: CustomerId,
    /// The plan being subscribed to.
    pub plan_id: PlanId,
    /// The linked Stripe subscription id.
    pub stripe_subscription_id: String,
    /// The linked Stripe subscription item id.
    pub stripe_subscription_item_id: String,
    /// The current lifecycle state.
    pub status: SubscriptionStatus,
    /// Start of the current billing period.
    pub current_period_start: OffsetDateTime,
    /// End of the current billing period.
    pub current_period_end: OffsetDateTime,
    /// Whether the subscription is set to end at the period boundary.
    pub cancel_at_period_end: bool,
    /// Stripe's `created` timestamp of the last webhook event applied to this
    /// row. `None` means no event has been applied
    /// yet -- true of every row created before webhook processing existed.
    /// A later event is applied only when its `created` is `None`-relative
    /// (always applies) or greater than or equal to this value; an older
    /// event is recorded but not applied, which is what stops an
    /// out-of-order redelivery from regressing the row.
    pub last_event_created_at: Option<OffsetDateTime>,
    /// When the subscription was created.
    pub created_at: OffsetDateTime,
    /// When the subscription was soft-deleted, if at all.
    pub deleted_at: Option<OffsetDateTime>,
}

/// Port for persisting and querying `Subscription` records. Implemented by an
/// adapter crate (`persistence`); no I/O here.
///
/// Written as `fn … -> impl Future<Output = …> + Send` rather than bare
/// `async fn`, matching `OutboundRequestRepository` and
/// `WebhookEventRepository`: `service`'s generic `WebhookProcessor` goes
/// behind `#[async_trait]` to implement the object-safe
/// `WebhookHandler` port, which boxes its futures as `Send`, so every future
/// it awaits -- including these -- must be `Send` too. A bare `async fn` in a
/// trait does not promise that for a generic `S`. Implementors may still
/// write `async fn` in the `impl` block; the bound is checked there.
pub trait SubscriptionRepository {
    /// Creates a new subscription for the given tenant. `cancel_at_period_end`
    /// starts `false`.
    #[allow(clippy::too_many_arguments)]
    fn create(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        plan_id: PlanId,
        stripe_subscription_id: String,
        stripe_subscription_item_id: String,
        status: SubscriptionStatus,
        current_period_start: OffsetDateTime,
        current_period_end: OffsetDateTime,
    ) -> impl Future<Output = Result<Subscription, DomainError>> + Send;

    /// Finds a subscription by id, scoped to the tenant. Returns `None` if it
    /// does not exist or has been soft-deleted.
    fn find(
        &self,
        tenant_id: TenantId,
        id: SubscriptionId,
    ) -> impl Future<Output = Result<Option<Subscription>, DomainError>> + Send;

    /// Lists all subscriptions for the given tenant, excluding soft-deleted ones.
    fn list(
        &self,
        tenant_id: TenantId,
    ) -> impl Future<Output = Result<Vec<Subscription>, DomainError>> + Send;

    /// Finds a subscription by its Stripe id, scoped to the tenant. Returns
    /// `None` if it does not exist, has been soft-deleted, or belongs to a
    /// different tenant. Tenant-scoped, unlike
    /// [`CustomerRepository::find_by_stripe_customer_id`](crate::CustomerRepository::find_by_stripe_customer_id):
    /// the webhook path always has a `Customer` -- and therefore
    /// a `TenantId` -- in hand by the time it needs this lookup, so this is
    /// not the tenant-establishing exception that one is.
    fn find_by_stripe_subscription_id(
        &self,
        tenant_id: TenantId,
        stripe_subscription_id: &str,
    ) -> impl Future<Output = Result<Option<Subscription>, DomainError>> + Send;

    /// Applies a webhook event's effect to a subscription row, guarded by
    /// the ordering rule: the write is admitted when
    /// `event_created_at` is greater than or equal to the row's current
    /// `last_event_created_at` (or that column is `NULL`), and rejected --
    /// recorded as [`EventApplication::Stale`], row unchanged -- otherwise.
    ///
    /// **The guard is enforced in the `UPDATE`'s `WHERE` clause, not by a
    /// read-then-compare in Rust.** Two concurrent deliveries processed in
    /// Rust could both read the same stale `last_event_created_at`, both
    /// decide they are newer, and the older of the two could win the race to
    /// write last. Putting the predicate in the statement means Postgres
    /// resolves it atomically, and `rows_affected() == 0` *is* the stale
    /// signal.
    ///
    /// Writes `status`, `current_period_start`, `current_period_end`,
    /// `cancel_at_period_end` and `last_event_created_at` together, in one
    /// statement -- never `plan_id`, which this method does not accept.
    ///
    /// **Precondition:** the row identified by `(tenant_id, id)` exists and
    /// is not soft-deleted. Callers reach this method after already having
    /// looked the row up (e.g. via `find_by_stripe_subscription_id`), so a
    /// zero-rows result is read as "a newer event already applied" rather
    /// than "no such row" -- this method cannot tell the two apart, and does
    /// not need to for its one caller.
    #[allow(clippy::too_many_arguments)]
    fn apply_event(
        &self,
        tenant_id: TenantId,
        id: SubscriptionId,
        status: SubscriptionStatus,
        current_period_start: OffsetDateTime,
        current_period_end: OffsetDateTime,
        cancel_at_period_end: bool,
        event_created_at: OffsetDateTime,
    ) -> impl Future<Output = Result<EventApplication, DomainError>> + Send;

    /// Repoints a subscription at a different local plan.
    ///
    /// **Deliberately separate from [`apply_event`](Self::apply_event), and
    /// deliberately *not* guarded by the webhook ordering rule.** The two write
    /// disjoint columns for different reasons:
    ///
    /// - `apply_event` writes what *Stripe* reported (status, period bounds,
    ///   `cancel_at_period_end`), so it must be ordered against other Stripe
    ///   events, and it cannot accept a `plan_id` — a webhook payload names a
    ///   Stripe *price*, and the `updated` path has no local plan to write.
    /// - `plan_id` is the one column no webhook ever writes. Its only source
    ///   is a caller who explicitly asked for *this* local plan
    ///   (`POST /subscriptions/{id}/change-plan`), so there is no older or
    ///   newer event to lose a race against, and an ordering predicate would
    ///   only be able to reject the write that is by definition authoritative.
    ///
    /// Call it **after** the corresponding Stripe call has succeeded (Stripe
    /// is authoritative, the local table is a cache). Concurrent plan
    /// changes are last-write-wins, which is what Stripe itself does.
    ///
    /// **Precondition:** the row identified by `(tenant_id, id)` exists and
    /// is not soft-deleted — callers reach this after a tenant-scoped `find`,
    /// the same precondition `apply_event` documents. A row that does not
    /// match is silently not updated rather than an error, for that reason.
    fn set_plan(
        &self,
        tenant_id: TenantId,
        id: SubscriptionId,
        plan_id: PlanId,
    ) -> impl Future<Output = Result<(), DomainError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_as_uuid() {
        let id = Uuid::new_v4();
        let subscription_id = SubscriptionId::new(id);
        assert_eq!(subscription_id.as_uuid(), id);
    }

    #[test]
    fn status_round_trips_through_str() {
        for status in [
            SubscriptionStatus::Active,
            SubscriptionStatus::PastDue,
            SubscriptionStatus::Canceled,
            SubscriptionStatus::Incomplete,
            SubscriptionStatus::IncompleteExpired,
        ] {
            assert_eq!(SubscriptionStatus::try_from(status.as_str()), Ok(status));
        }
    }

    #[test]
    fn unknown_status_is_rejected() {
        assert!(SubscriptionStatus::try_from("trialing").is_err());
    }
}
