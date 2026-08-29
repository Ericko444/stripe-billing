use core::fmt;

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
/// subscription statuses this system acts on (`init-spec.md` §10.4). Stored
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
}

impl SubscriptionStatus {
    /// The wire/storage form, matching Stripe's own strings.
    pub fn as_str(&self) -> &'static str {
        match self {
            SubscriptionStatus::Active => "active",
            SubscriptionStatus::PastDue => "past_due",
            SubscriptionStatus::Canceled => "canceled",
            SubscriptionStatus::Incomplete => "incomplete",
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
            other => Err(DomainError::Repository(format!(
                "unknown subscription status: {other:?}"
            ))),
        }
    }
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
    /// When the subscription was created.
    pub created_at: OffsetDateTime,
    /// When the subscription was soft-deleted, if at all.
    pub deleted_at: Option<OffsetDateTime>,
}

/// Port for persisting and querying `Subscription` records. Implemented by an
/// adapter crate (`persistence`); no I/O here.
#[allow(async_fn_in_trait)]
pub trait SubscriptionRepository {
    /// Creates a new subscription for the given tenant. `cancel_at_period_end`
    /// starts `false`; nothing in Phase 1 sets it.
    #[allow(clippy::too_many_arguments)]
    async fn create(
        &self,
        tenant_id: TenantId,
        customer_id: CustomerId,
        plan_id: PlanId,
        stripe_subscription_id: String,
        stripe_subscription_item_id: String,
        status: SubscriptionStatus,
        current_period_start: OffsetDateTime,
        current_period_end: OffsetDateTime,
    ) -> Result<Subscription, DomainError>;

    /// Finds a subscription by id, scoped to the tenant. Returns `None` if it
    /// does not exist or has been soft-deleted.
    async fn find(
        &self,
        tenant_id: TenantId,
        id: SubscriptionId,
    ) -> Result<Option<Subscription>, DomainError>;

    /// Lists all subscriptions for the given tenant, excluding soft-deleted ones.
    async fn list(&self, tenant_id: TenantId) -> Result<Vec<Subscription>, DomainError>;
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
        ] {
            assert_eq!(SubscriptionStatus::try_from(status.as_str()), Ok(status));
        }
    }

    #[test]
    fn unknown_status_is_rejected() {
        assert!(SubscriptionStatus::try_from("trialing").is_err());
    }
}
