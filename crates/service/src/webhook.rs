use async_trait::async_trait;
use domain::{
    BillingEvent, BillingEventSink, CustomerRepository, DomainError, SubscriptionRepository,
    VerifiedEvent, WebhookEventRepository,
};

use crate::subscription_lifecycle;

/// The outcome of processing one verified webhook event (`init-spec.md`
/// §10.2, spec decision 3).
///
/// `Applied` carries the [`BillingEvent`] the mirror write produced, rather
/// than being a bare unit variant: the same reasoning as
/// `WebhookReceipt::Fresh` carrying a `VerifiedEvent` while `Duplicate`
/// carries nothing. It is the state that has something to hand the sink, so
/// it carries that something -- `handle` cannot call the sink with the
/// wrong event, or forget to, because there is nowhere else to get one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventOutcome {
    /// The mirror was updated; this is what was notified to the sink.
    Applied(BillingEvent),
    /// Verified and recorded, deliberately not applied.
    NotApplied(NotAppliedReason),
}

/// Why [`WebhookHandler::handle`] did nothing, despite the event having
/// verified and been recorded.
///
/// **None of these are errors.** Every one still returns 200 at the route
/// (a later phase's `api`): collapsing them into `Ok(())` would make the
/// first production question -- "why isn't my subscription updating?" --
/// unanswerable from logs. `UnknownCustomer` in particular is not a failure:
/// it means the event concerns a Stripe customer this module does not own,
/// and a 5xx would make Stripe retry forever over data that will never
/// exist here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAppliedReason {
    /// Older than the last event applied to this row (§10.2).
    Stale,
    /// No local customer row for the Stripe customer id (§10.3).
    UnknownCustomer,
    /// No local mirror row for the Stripe subscription id.
    UnknownSubscription,
    /// No handler for this event type -- acknowledged, never rejected
    /// (§10.4).
    UnhandledType,
}

/// Object-safe handler for one verified, deduplicated webhook event.
///
/// A later phase's `AppState` holds this as `Arc<dyn WebhookHandler>` rather
/// than the generic [`WebhookProcessor<C, S, W, K>`] directly, so the router
/// factory does not have to carry four type parameters through its
/// signature -- the same trade `BillingProvider` and `WebhookVerifier` made.
#[async_trait]
pub trait WebhookHandler: Send + Sync {
    /// Processes one verified event, reporting why nothing changed when
    /// nothing did.
    async fn handle(&self, event: VerifiedEvent) -> Result<EventOutcome, DomainError>;
}

/// Orchestrates webhook processing over the four ports it needs: customer
/// and subscription lookups, the webhook ledger, and the host's event sink.
///
/// Generic over all four, matching `StripeBillingProvider<R>`: the compiler
/// monomorphises the real wiring `demo` picks, and nothing is boxed on the
/// hot path. Implements the object-safe [`WebhookHandler`] below so a later
/// phase's `AppState` can hold it as `Arc<dyn WebhookHandler>` instead.
pub struct WebhookProcessor<C, S, W, K> {
    customers: C,
    subscriptions: S,
    webhook_events: W,
    sink: K,
}

impl<C, S, W, K> WebhookProcessor<C, S, W, K> {
    /// Wraps the four ports webhook processing needs.
    pub fn new(customers: C, subscriptions: S, webhook_events: W, sink: K) -> Self {
        Self {
            customers,
            subscriptions,
            webhook_events,
            sink,
        }
    }
}

#[async_trait]
impl<C, S, W, K> WebhookHandler for WebhookProcessor<C, S, W, K>
where
    C: CustomerRepository + Send + Sync,
    S: SubscriptionRepository + Send + Sync,
    W: WebhookEventRepository + Send + Sync,
    K: BillingEventSink,
{
    async fn handle(&self, event: VerifiedEvent) -> Result<EventOutcome, DomainError> {
        // No match on anything but the type string. An unrecognised type is
        // acknowledged, never rejected (§10.4).
        let outcome = match event.event_type.as_str() {
            "customer.subscription.updated" => {
                subscription_lifecycle::apply(&self.customers, &self.subscriptions, &event).await?
            }
            _ => EventOutcome::NotApplied(NotAppliedReason::UnhandledType),
        };

        // The sink is a side effect: it runs after the mirror is a fact
        // (the match above already committed the write), and its failure
        // does not undo the mirror (§8.3). It is never called for a
        // NotApplied outcome -- there is nothing to notify the host about.
        if let EventOutcome::Applied(ref billing_event) = outcome
            && let Err(err) = self.sink.handle(billing_event.clone()).await
        {
            tracing::warn!(
                error = %err,
                stripe_event_id = %event.stripe_event_id,
                "billing event sink failed; mirror retained, processed_at left NULL"
            );
            // processed_at stays NULL: processing did not finish, and that
            // is exactly what lets a later recovery sweep over processed_at
            // IS NULL find this event again (decision 5).
            return Ok(outcome);
        }

        // Nothing further will ever happen to a NotApplied event -- of any
        // reason, including UnhandledType -- so it is marked processed even
        // though nothing was applied (decision 5).
        self.webhook_events.mark_processed(event.id).await?;
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use domain::{
        Customer, CustomerId, Subscription, SubscriptionId, TenantId, WebhookEvent, WebhookEventId,
    };
    use serde_json::{Value, json};
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{
        InMemoryCustomers, InMemorySink, InMemorySubscriptions, InMemoryWebhookEvents,
    };

    /// Compiles only if `WebhookHandler` is dyn-compatible -- the property
    /// the `#[async_trait]` decision exists for.
    #[allow(dead_code)]
    fn assert_dyn_compatible(_handler: &dyn WebhookHandler) {}

    type TestProcessor = WebhookProcessor<
        InMemoryCustomers,
        InMemorySubscriptions,
        InMemoryWebhookEvents,
        InMemorySink,
    >;

    fn processor() -> TestProcessor {
        WebhookProcessor::new(
            InMemoryCustomers::default(),
            InMemorySubscriptions::default(),
            InMemoryWebhookEvents::default(),
            InMemorySink::default(),
        )
    }

    fn verified_event(event_type: &str) -> VerifiedEvent {
        VerifiedEvent {
            id: WebhookEventId::new(Uuid::new_v4()),
            stripe_event_id: format!("evt_{}", Uuid::new_v4()),
            event_type: event_type.to_string(),
            created: OffsetDateTime::now_utc(),
            payload: json!({}),
        }
    }

    #[tokio::test]
    async fn unrecognised_event_type_is_not_applied_and_never_errors() {
        let processor = processor();
        let event = verified_event("some.future.event.type");
        processor.webhook_events.seed(WebhookEvent {
            id: event.id,
            tenant_id: None,
            stripe_event_id: event.stripe_event_id.clone(),
            event_type: event.event_type.clone(),
            payload: event.payload.clone(),
            created_at: event.created,
            processed_at: None,
        });

        let result = processor.handle(event.clone()).await;

        assert_eq!(
            result,
            Ok(EventOutcome::NotApplied(NotAppliedReason::UnhandledType))
        );
    }

    #[tokio::test]
    async fn unhandled_type_still_marks_the_event_processed() {
        let processor = processor();
        let event = verified_event("some.future.event.type");
        processor.webhook_events.seed(WebhookEvent {
            id: event.id,
            tenant_id: None,
            stripe_event_id: event.stripe_event_id.clone(),
            event_type: event.event_type.clone(),
            payload: event.payload.clone(),
            created_at: event.created,
            processed_at: None,
        });

        assert_eq!(processor.webhook_events.processed_at(event.id), Some(None));

        let _ = processor.handle(event.clone()).await;

        assert!(matches!(
            processor.webhook_events.processed_at(event.id),
            Some(Some(_))
        ));
    }

    /// Builds a `customer.subscription.updated` payload with the shape
    /// `subscription_lifecycle::read_subscription` expects, for a given
    /// Stripe customer and subscription id.
    fn subscription_updated_payload(
        stripe_customer_id: &str,
        stripe_subscription_id: &str,
    ) -> Value {
        let now = OffsetDateTime::now_utc();
        json!({
            "id": stripe_subscription_id,
            "type": "customer.subscription.updated",
            "data": {
                "object": {
                    "id": stripe_subscription_id,
                    "customer": stripe_customer_id,
                    "status": "active",
                    "cancel_at_period_end": false,
                    "items": {
                        "data": [
                            {
                                "id": "si_test",
                                "current_period_start": now.unix_timestamp(),
                                "current_period_end": (now + Duration::days(30)).unix_timestamp(),
                            }
                        ]
                    }
                }
            }
        })
    }

    /// Seeds a customer and a matching subscription mirror row on
    /// `processor`, and returns the tenant/subscription ids plus a
    /// `VerifiedEvent` that will apply cleanly against them.
    fn seeded_applied_event(
        processor: &TestProcessor,
    ) -> (TenantId, SubscriptionId, VerifiedEvent) {
        let tenant_id = TenantId::new(Uuid::new_v4());
        let customer = Customer {
            id: CustomerId::new(Uuid::new_v4()),
            tenant_id,
            stripe_customer_id: Some("cus_applied".to_string()),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        };
        processor.customers.seed(customer.clone());

        let now = OffsetDateTime::now_utc();
        let subscription = Subscription {
            id: SubscriptionId::new(Uuid::new_v4()),
            tenant_id,
            customer_id: customer.id,
            plan_id: domain::PlanId::new(Uuid::new_v4()),
            stripe_subscription_id: "sub_applied".to_string(),
            stripe_subscription_item_id: "si_original".to_string(),
            status: domain::SubscriptionStatus::Incomplete,
            current_period_start: now,
            current_period_end: now + Duration::days(30),
            cancel_at_period_end: false,
            last_event_created_at: None,
            created_at: now,
            deleted_at: None,
        };
        processor.subscriptions.seed(subscription.clone());

        let payload = subscription_updated_payload("cus_applied", "sub_applied");
        let event = VerifiedEvent {
            id: WebhookEventId::new(Uuid::new_v4()),
            stripe_event_id: "evt_applied".to_string(),
            event_type: "customer.subscription.updated".to_string(),
            created: OffsetDateTime::now_utc(),
            payload,
        };
        processor.webhook_events.seed(WebhookEvent {
            id: event.id,
            tenant_id: Some(tenant_id),
            stripe_event_id: event.stripe_event_id.clone(),
            event_type: event.event_type.clone(),
            payload: event.payload.clone(),
            created_at: event.created,
            processed_at: None,
        });

        (tenant_id, subscription.id, event)
    }

    #[tokio::test]
    async fn applied_event_notifies_the_sink_exactly_once_and_marks_processed() {
        let processor = processor();
        let (_, _, event) = seeded_applied_event(&processor);

        let outcome = processor.handle(event.clone()).await;

        assert!(matches!(outcome, Ok(EventOutcome::Applied(_))));
        assert_eq!(processor.sink.received().len(), 1);
        assert!(matches!(
            processor.webhook_events.processed_at(event.id),
            Some(Some(_))
        ));
    }

    #[tokio::test]
    async fn sink_is_not_called_for_a_not_applied_outcome() {
        let processor = processor();
        let event = verified_event("some.future.event.type");
        processor.webhook_events.seed(WebhookEvent {
            id: event.id,
            tenant_id: None,
            stripe_event_id: event.stripe_event_id.clone(),
            event_type: event.event_type.clone(),
            payload: event.payload.clone(),
            created_at: event.created,
            processed_at: None,
        });

        let _ = processor.handle(event).await;

        assert_eq!(processor.sink.received().len(), 0);
    }

    #[tokio::test]
    async fn failing_sink_leaves_the_mirror_written_and_processed_at_null() {
        let processor = TestProcessor::new(
            InMemoryCustomers::default(),
            InMemorySubscriptions::default(),
            InMemoryWebhookEvents::default(),
            InMemorySink::failing(),
        );
        let (tenant_id, subscription_id, event) = seeded_applied_event(&processor);

        let outcome = processor.handle(event.clone()).await;

        assert!(matches!(outcome, Ok(EventOutcome::Applied(_))));
        // The mirror write already committed before the sink was ever
        // called -- it is retained regardless of the sink's outcome (§8.3).
        let found = processor
            .subscriptions
            .find(tenant_id, subscription_id)
            .await;
        assert!(matches!(
            found,
            Ok(Some(ref s)) if s.status == domain::SubscriptionStatus::Active
        ));
        // processing did not finish, so processed_at stays NULL -- this is
        // what makes a later recovery sweep able to find it (decision 5).
        assert_eq!(processor.webhook_events.processed_at(event.id), Some(None));
    }
}
