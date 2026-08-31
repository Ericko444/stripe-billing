use domain::{
    BillingEvent, CustomerRepository, DomainError, EventApplication, SubscriptionId,
    SubscriptionRepository, SubscriptionStatus, TenantId, VerifiedEvent,
};
use serde_json::Value;
use time::OffsetDateTime;

use crate::webhook::{EventOutcome, NotAppliedReason};

/// Applies a `customer.subscription.updated` event: resolves the tenant from
/// the Stripe customer id (§10.3), locates the local subscription mirror by
/// its Stripe id, and applies the event through the ordering guard (§10.2).
///
/// Tenant resolution reads `data.object.customer` -- Stripe's *own*
/// identifier for the object -- and looks it up in the database via
/// [`CustomerRepository::find_by_stripe_customer_id`]. No code path here
/// reads a tenant id from the payload: the payload has no opinion about
/// *our* tenancy, only about Stripe's own objects, and this module never
/// treats it as authority for anything else.
///
/// `plan_id` is never touched -- a plan change from this event type is out
/// of scope for this slice (spec Open Question 1); only status, both period
/// bounds and `cancel_at_period_end` are applied.
pub async fn apply<C, S>(
    customers: &C,
    subscriptions: &S,
    event: &VerifiedEvent,
) -> Result<EventOutcome, DomainError>
where
    C: CustomerRepository,
    S: SubscriptionRepository,
{
    let fields = read_subscription(&event.payload)?;

    let Some(customer) = customers
        .find_by_stripe_customer_id(&fields.stripe_customer_id)
        .await?
    else {
        // Not a failure: this event concerns a Stripe customer this module
        // does not own (§10.3).
        return Ok(EventOutcome::NotApplied(NotAppliedReason::UnknownCustomer));
    };

    let Some(subscription) = subscriptions
        .find_by_stripe_subscription_id(customer.tenant_id, &fields.stripe_subscription_id)
        .await?
    else {
        return Ok(EventOutcome::NotApplied(
            NotAppliedReason::UnknownSubscription,
        ));
    };

    let application = subscriptions
        .apply_event(
            customer.tenant_id,
            subscription.id,
            fields.status,
            fields.current_period_start,
            fields.current_period_end,
            fields.cancel_at_period_end,
            event.created,
        )
        .await?;

    Ok(match application {
        EventApplication::Applied => EventOutcome::Applied(billing_event_for(
            customer.tenant_id,
            subscription.id,
            fields.status,
        )),
        EventApplication::Stale => EventOutcome::NotApplied(NotAppliedReason::Stale),
    })
}

/// Maps the subscription's new status to the [`BillingEvent`] this handler
/// emits (§8.3's translation from a Stripe event into a host-facing typed
/// notification). Not spec-mandated -- the spec fixes `BillingEvent`'s
/// shape and the ordering around calling the sink, not which event each
/// status produces -- so the mapping is deliberately the smallest one that
/// covers the three variants `service` currently defines: `Active` becomes
/// `SubscriptionActivated` (a new subscriber, or a recovery from
/// `past_due`/`incomplete`); `Canceled` becomes `SubscriptionCanceled`;
/// everything else (a period rollover, a `cancel_at_period_end` flip with
/// no status change, `PastDue`, `Incomplete`) falls to the catch-all
/// `SubscriptionUpdated`.
fn billing_event_for(
    tenant_id: TenantId,
    subscription_id: SubscriptionId,
    status: SubscriptionStatus,
) -> BillingEvent {
    match status {
        SubscriptionStatus::Active => BillingEvent::SubscriptionActivated {
            tenant_id,
            subscription_id,
        },
        SubscriptionStatus::Canceled => BillingEvent::SubscriptionCanceled {
            tenant_id,
            subscription_id,
        },
        SubscriptionStatus::PastDue | SubscriptionStatus::Incomplete => {
            BillingEvent::SubscriptionUpdated {
                tenant_id,
                subscription_id,
            }
        }
    }
}

/// The fields this handler needs off a `customer.subscription.updated`
/// envelope's `data.object`.
struct SubscriptionFields {
    stripe_customer_id: String,
    stripe_subscription_id: String,
    status: SubscriptionStatus,
    current_period_start: OffsetDateTime,
    current_period_end: OffsetDateTime,
    cancel_at_period_end: bool,
}

/// Reads the fields this handler needs from the raw event payload's
/// `data.object`, through `Value` accessors rather than a `serde` derive --
/// the crate takes no dependency beyond `serde_json`. A missing or
/// wrong-typed field is a [`DomainError::MalformedEvent`], never an index
/// panic: this payload already passed signature verification, but verified
/// only means the signature matched, not that its shape is what this code
/// expects.
///
/// `current_period_start` / `current_period_end` are read off the
/// subscription's *first item*, not top-level fields -- in the API version
/// this workspace is pinned to those two fields live on the item, alongside
/// its id (see `stripe-adapter::subscriptions::snapshot_from`, which reads
/// the identical shape from Stripe's typed response). A subscription with
/// no items is a `MalformedEvent`, the same call `snapshot_from` makes for
/// the equivalent REST response case.
fn read_subscription(payload: &Value) -> Result<SubscriptionFields, DomainError> {
    let object = payload
        .get("data")
        .and_then(|data| data.get("object"))
        .ok_or_else(|| {
            DomainError::MalformedEvent("event envelope missing data.object".to_string())
        })?;

    let field_str = |name: &str| -> Result<String, DomainError> {
        object
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                DomainError::MalformedEvent(format!("subscription object missing string `{name}`"))
            })
    };

    let stripe_customer_id = field_str("customer")?;
    let stripe_subscription_id = field_str("id")?;
    let status = SubscriptionStatus::try_from(field_str("status")?.as_str())?;
    let cancel_at_period_end = object
        .get("cancel_at_period_end")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            DomainError::MalformedEvent(
                "subscription object missing bool `cancel_at_period_end`".to_string(),
            )
        })?;

    let item = object
        .get("items")
        .and_then(|items| items.get("data"))
        .and_then(|data| data.get(0))
        .ok_or_else(|| {
            DomainError::MalformedEvent("subscription object carried no items".to_string())
        })?;

    let field_i64 = |name: &str| -> Result<i64, DomainError> {
        item.get(name).and_then(Value::as_i64).ok_or_else(|| {
            DomainError::MalformedEvent(format!("subscription item missing integer `{name}`"))
        })
    };

    let to_offset = |ts: i64| {
        OffsetDateTime::from_unix_timestamp(ts).map_err(|err| {
            DomainError::MalformedEvent(format!("timestamp {ts} out of range: {err}"))
        })
    };

    let current_period_start = to_offset(field_i64("current_period_start")?)?;
    let current_period_end = to_offset(field_i64("current_period_end")?)?;

    Ok(SubscriptionFields {
        stripe_customer_id,
        stripe_subscription_id,
        status,
        current_period_start,
        current_period_end,
        cancel_at_period_end,
    })
}

#[cfg(test)]
mod tests {
    use domain::{Customer, CustomerId, Subscription, SubscriptionId, TenantId};
    use serde_json::json;
    use time::Duration;
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{InMemoryCustomers, InMemorySubscriptions};

    fn event_payload(
        stripe_customer_id: &str,
        stripe_subscription_id: &str,
        status: &str,
        current_period_start: i64,
        current_period_end: i64,
        cancel_at_period_end: bool,
    ) -> Value {
        json!({
            "id": "evt_test",
            "type": "customer.subscription.updated",
            "data": {
                "object": {
                    "id": stripe_subscription_id,
                    "customer": stripe_customer_id,
                    "status": status,
                    "cancel_at_period_end": cancel_at_period_end,
                    "items": {
                        "data": [
                            {
                                "id": "si_test",
                                "current_period_start": current_period_start,
                                "current_period_end": current_period_end,
                            }
                        ]
                    }
                }
            }
        })
    }

    fn verified_event(payload: Value, created: OffsetDateTime) -> VerifiedEvent {
        VerifiedEvent {
            id: domain::WebhookEventId::new(Uuid::new_v4()),
            stripe_event_id: "evt_test".to_string(),
            event_type: "customer.subscription.updated".to_string(),
            created,
            payload,
        }
    }

    /// Seeds one tenant with a customer and a matching subscription mirror
    /// row, returning the ids the event payload should reference.
    fn seed_tenant_with_subscription(
        customers: &InMemoryCustomers,
        subscriptions: &InMemorySubscriptions,
        stripe_customer_id: &str,
        stripe_subscription_id: &str,
    ) -> (TenantId, SubscriptionId) {
        let tenant_id = TenantId::new(Uuid::new_v4());
        let customer = Customer {
            id: CustomerId::new(Uuid::new_v4()),
            tenant_id,
            stripe_customer_id: Some(stripe_customer_id.to_string()),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        };
        customers.seed(customer.clone());

        let now = OffsetDateTime::now_utc();
        let subscription = Subscription {
            id: SubscriptionId::new(Uuid::new_v4()),
            tenant_id,
            customer_id: customer.id,
            plan_id: domain::PlanId::new(Uuid::new_v4()),
            stripe_subscription_id: stripe_subscription_id.to_string(),
            stripe_subscription_item_id: "si_original".to_string(),
            status: SubscriptionStatus::Incomplete,
            current_period_start: now,
            current_period_end: now + Duration::days(30),
            cancel_at_period_end: false,
            last_event_created_at: None,
            created_at: now,
            deleted_at: None,
        };
        subscriptions.seed(subscription.clone());

        (tenant_id, subscription.id)
    }

    #[tokio::test]
    async fn valid_event_updates_the_mirror() {
        let customers = InMemoryCustomers::default();
        let subscriptions = InMemorySubscriptions::default();
        let (tenant_id, subscription_id) =
            seed_tenant_with_subscription(&customers, &subscriptions, "cus_1", "sub_1");

        let created = OffsetDateTime::now_utc();
        let payload = event_payload(
            "cus_1",
            "sub_1",
            "active",
            created.unix_timestamp(),
            (created + Duration::days(30)).unix_timestamp(),
            true,
        );
        let event = verified_event(payload, created);

        let outcome = apply(&customers, &subscriptions, &event).await;

        assert_eq!(
            outcome,
            Ok(EventOutcome::Applied(BillingEvent::SubscriptionActivated {
                tenant_id,
                subscription_id,
            }))
        );
        let found = subscriptions.find(tenant_id, subscription_id).await;
        assert!(matches!(
            found,
            Ok(Some(ref s)) if s.status == SubscriptionStatus::Active && s.cancel_at_period_end
        ));
    }

    #[tokio::test]
    async fn stale_event_does_not_regress_the_row() {
        let customers = InMemoryCustomers::default();
        let subscriptions = InMemorySubscriptions::default();
        let (tenant_id, subscription_id) =
            seed_tenant_with_subscription(&customers, &subscriptions, "cus_2", "sub_2");

        let newer = OffsetDateTime::now_utc();
        let newer_payload = event_payload(
            "cus_2",
            "sub_2",
            "active",
            newer.unix_timestamp(),
            (newer + Duration::days(30)).unix_timestamp(),
            false,
        );
        let newer_event = verified_event(newer_payload, newer);
        let first = apply(&customers, &subscriptions, &newer_event).await;
        assert_eq!(
            first,
            Ok(EventOutcome::Applied(BillingEvent::SubscriptionActivated {
                tenant_id,
                subscription_id,
            }))
        );

        let older = newer - Duration::minutes(5);
        let older_payload = event_payload(
            "cus_2",
            "sub_2",
            "canceled",
            older.unix_timestamp(),
            (older + Duration::days(1)).unix_timestamp(),
            true,
        );
        let older_event = verified_event(older_payload, older);

        let outcome = apply(&customers, &subscriptions, &older_event).await;

        assert_eq!(
            outcome,
            Ok(EventOutcome::NotApplied(NotAppliedReason::Stale))
        );
        let found = subscriptions.find(tenant_id, subscription_id).await;
        assert!(matches!(
            found,
            Ok(Some(ref s)) if s.status == SubscriptionStatus::Active && !s.cancel_at_period_end
        ));
    }

    #[tokio::test]
    async fn unknown_customer_is_not_applied_and_skips_subscription_lookup() {
        let customers = InMemoryCustomers::default();
        let subscriptions = InMemorySubscriptions::default();
        // Nothing seeded: the Stripe customer id resolves to no local row.

        let created = OffsetDateTime::now_utc();
        let payload = event_payload(
            "cus_unknown",
            "sub_unknown",
            "active",
            created.unix_timestamp(),
            (created + Duration::days(30)).unix_timestamp(),
            false,
        );
        let event = verified_event(payload, created);

        let outcome = apply(&customers, &subscriptions, &event).await;

        assert_eq!(
            outcome,
            Ok(EventOutcome::NotApplied(NotAppliedReason::UnknownCustomer))
        );
    }

    #[tokio::test]
    async fn unknown_subscription_is_not_applied() {
        let customers = InMemoryCustomers::default();
        let subscriptions = InMemorySubscriptions::default();
        let tenant_id = TenantId::new(Uuid::new_v4());
        let customer = Customer {
            id: CustomerId::new(Uuid::new_v4()),
            tenant_id,
            stripe_customer_id: Some("cus_3".to_string()),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        };
        customers.seed(customer);
        // No subscription seeded for "sub_3".

        let created = OffsetDateTime::now_utc();
        let payload = event_payload(
            "cus_3",
            "sub_3",
            "active",
            created.unix_timestamp(),
            (created + Duration::days(30)).unix_timestamp(),
            false,
        );
        let event = verified_event(payload, created);

        let outcome = apply(&customers, &subscriptions, &event).await;

        assert_eq!(
            outcome,
            Ok(EventOutcome::NotApplied(
                NotAppliedReason::UnknownSubscription
            ))
        );
    }
}
