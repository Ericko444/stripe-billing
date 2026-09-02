use domain::{
    CancellationTiming, DomainError, OutboundRequestRepository, SubscriptionSnapshot,
    SubscriptionStatus, TenantId,
};
use stripe::{RequestStrategy, StripeRequest};
use stripe_billing::subscription::{
    CancelSubscription, CreateSubscription, CreateSubscriptionItems,
    CreateSubscriptionPaymentBehavior, UpdateSubscription, UpdateSubscriptionItems,
    UpdateSubscriptionPaymentBehavior, UpdateSubscriptionProrationBehavior,
};
use time::OffsetDateTime;

use crate::Reservation;
use crate::ledger::{Ledger, idempotency_key};
use crate::{StripeError, fingerprint};

/// `BillingProvider::create_subscription`'s real implementation. Same shape
/// as the customer methods: fingerprint the inputs, reserve an idempotency
/// key, call Stripe with it, mark the reservation complete, return a
/// snapshot.
///
/// Both inputs -- the customer id and the price id -- vary the request body,
/// so both are in the fingerprint.
///
/// `payment_behavior: default_incomplete` is deliberate, not Stripe's
/// unstated default: without it, a customer with no payment method makes
/// Stripe **reject the request outright** ("no attached payment source")
/// rather than create the subscription `incomplete` -- confirmed against a
/// real test-mode account while building Phase 4d's seed (Plan 4d, P4),
/// which depends on exactly the `incomplete`-not-rejected behavior this line
/// guarantees.
pub async fn create_subscription<R: OutboundRequestRepository>(
    client: &stripe::Client,
    ledger: &Ledger<R>,
    tenant_id: TenantId,
    stripe_customer_id: &str,
    stripe_price_id: &str,
) -> Result<SubscriptionSnapshot, DomainError> {
    let request_fingerprint = fingerprint(
        "create_subscription",
        &[stripe_customer_id, stripe_price_id],
    );

    let reservation = ledger
        .reserve(tenant_id, "create_subscription", &request_fingerprint)
        .await?;

    let items = vec![CreateSubscriptionItems {
        price: Some(stripe_price_id.to_string()),
        ..CreateSubscriptionItems::new()
    }];
    let subscription = CreateSubscription::new()
        .customer(stripe_customer_id)
        .items(items)
        .payment_behavior(CreateSubscriptionPaymentBehavior::DefaultIncomplete)
        .customize()
        .request_strategy(RequestStrategy::Idempotent(idempotency_key(&reservation)?))
        .send(client)
        .await
        .map_err(StripeError::from)
        .map_err(DomainError::from)?;

    complete_with_snapshot(ledger, tenant_id, &reservation, subscription).await
}

/// `BillingProvider::change_plan`'s real implementation. Updates the
/// *existing* subscription item -- `items: [{ id, price }]` -- never adds a
/// second one, which is the double-billing bug `init-spec.md` §5.1 exists to
/// prevent. Prorates the change and errors (rather than leaving the
/// subscription incomplete) if the immediate payment fails (§7.3, Open
/// Question 1's resolved default).
///
/// The fingerprint covers the subscription id, the stored item id and the
/// new price -- the three inputs that vary the request.
pub async fn change_plan<R: OutboundRequestRepository>(
    client: &stripe::Client,
    ledger: &Ledger<R>,
    tenant_id: TenantId,
    stripe_subscription_id: &str,
    stripe_subscription_item_id: &str,
    new_stripe_price_id: &str,
) -> Result<SubscriptionSnapshot, DomainError> {
    let request_fingerprint = fingerprint(
        "change_plan",
        &[
            stripe_subscription_id,
            stripe_subscription_item_id,
            new_stripe_price_id,
        ],
    );

    let reservation = ledger
        .reserve(tenant_id, "change_plan", &request_fingerprint)
        .await?;

    let items = vec![UpdateSubscriptionItems {
        id: Some(stripe_subscription_item_id.to_string()),
        price: Some(new_stripe_price_id.to_string()),
        ..UpdateSubscriptionItems::new()
    }];
    let subscription = UpdateSubscription::new(stripe_subscription_id)
        .items(items)
        .proration_behavior(UpdateSubscriptionProrationBehavior::CreateProrations)
        .payment_behavior(UpdateSubscriptionPaymentBehavior::ErrorIfIncomplete)
        .customize()
        .request_strategy(RequestStrategy::Idempotent(idempotency_key(&reservation)?))
        .send(client)
        .await
        .map_err(StripeError::from)
        .map_err(DomainError::from)?;

    complete_with_snapshot(ledger, tenant_id, &reservation, subscription).await
}

/// `BillingProvider::cancel_subscription`'s real implementation. Two Stripe
/// operations behind one method: `AtPeriodEnd` is a subscription *update*
/// setting `cancel_at_period_end = true` (the subscription stays live until
/// the boundary); `Immediate` is a *delete*. The timing is in the
/// fingerprint, so the two are separate ledger rows with separate keys --
/// switching a pending cancellation from one to the other is a genuinely
/// different request, not a retry.
pub async fn cancel_subscription<R: OutboundRequestRepository>(
    client: &stripe::Client,
    ledger: &Ledger<R>,
    tenant_id: TenantId,
    stripe_subscription_id: &str,
    timing: CancellationTiming,
) -> Result<SubscriptionSnapshot, DomainError> {
    let timing_tag = match timing {
        CancellationTiming::AtPeriodEnd => "at_period_end",
        CancellationTiming::Immediate => "immediate",
    };
    let request_fingerprint =
        fingerprint("cancel_subscription", &[stripe_subscription_id, timing_tag]);

    let reservation = ledger
        .reserve(tenant_id, "cancel_subscription", &request_fingerprint)
        .await?;

    let key = idempotency_key(&reservation)?;
    let subscription = match timing {
        CancellationTiming::AtPeriodEnd => {
            UpdateSubscription::new(stripe_subscription_id)
                .cancel_at_period_end(true)
                .customize()
                .request_strategy(RequestStrategy::Idempotent(key))
                .send(client)
                .await
        }
        CancellationTiming::Immediate => {
            CancelSubscription::new(stripe_subscription_id)
                .customize()
                .request_strategy(RequestStrategy::Idempotent(key))
                .send(client)
                .await
        }
    }
    .map_err(StripeError::from)
    .map_err(DomainError::from)?;

    complete_with_snapshot(ledger, tenant_id, &reservation, subscription).await
}

/// Marks the reservation complete with the subscription's id and returns its
/// snapshot -- the tail every subscription method shares.
async fn complete_with_snapshot<R: OutboundRequestRepository>(
    ledger: &Ledger<R>,
    tenant_id: TenantId,
    reservation: &Reservation,
    subscription: stripe_billing::Subscription,
) -> Result<SubscriptionSnapshot, DomainError> {
    let snapshot = snapshot_from(subscription)?;
    ledger
        .complete(
            tenant_id,
            reservation,
            snapshot.stripe_subscription_id.clone(),
        )
        .await?;
    Ok(snapshot)
}

/// Turns Stripe's `Subscription` object into the port's `SubscriptionSnapshot`.
///
/// `stripe_subscription_item_id` and both period bounds come from the
/// subscription's *first item*, not from top-level fields: in the pinned API
/// version (`2026-07-29.dahlia`) `current_period_start` / `current_period_end`
/// live on the subscription item, alongside its id. A subscription with no
/// item is a response we cannot build a snapshot from -- a blank item id
/// would silently break a later `change_plan`, which is the double-billing
/// bug `init-spec.md` §5.1 exists to prevent -- so it is an error, never an
/// empty string.
fn snapshot_from(
    subscription: stripe_billing::Subscription,
) -> Result<SubscriptionSnapshot, DomainError> {
    let item = subscription.items.data.first().ok_or_else(|| {
        DomainError::from(StripeError::Deserialization(
            "subscription response carried no items; cannot resolve the \
             subscription item id or its billing period"
                .to_string(),
        ))
    })?;

    let to_offset = |ts: i64, field: &str| {
        OffsetDateTime::from_unix_timestamp(ts).map_err(|err| {
            DomainError::from(StripeError::Deserialization(format!(
                "subscription item {field} ({ts}) is not a valid timestamp: {err}"
            )))
        })
    };

    Ok(SubscriptionSnapshot {
        stripe_subscription_id: subscription.id.to_string(),
        stripe_subscription_item_id: item.id.to_string(),
        status: SubscriptionStatus::try_from(subscription.status.as_str())?,
        current_period_start: to_offset(item.current_period_start, "current_period_start")?,
        current_period_end: to_offset(item.current_period_end, "current_period_end")?,
        cancel_at_period_end: subscription.cancel_at_period_end,
    })
}
