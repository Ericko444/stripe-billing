use async_trait::async_trait;
use domain::{
    BillingEventSink, CustomerRepository, DomainError, SubscriptionRepository, VerifiedEvent,
    WebhookEventRepository,
};

use crate::subscription_updated;

/// The outcome of processing one verified webhook event (`init-spec.md`
/// §10.2, spec decision 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventOutcome {
    /// The mirror was updated and the sink notified.
    Applied,
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
///
/// `sink` becomes live once the next task wires in the mirror-write-then-
/// sink ordering (decision 4); until then it carries `#[allow(dead_code)]`,
/// the same idiom `assert_dyn_compatible` functions elsewhere in this
/// workspace use for a deliberately-for-now-unused item.
pub struct WebhookProcessor<C, S, W, K> {
    customers: C,
    subscriptions: S,
    webhook_events: W,
    #[allow(dead_code)]
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
                subscription_updated::apply(&self.customers, &self.subscriptions, &event).await?
            }
            _ => EventOutcome::NotApplied(NotAppliedReason::UnhandledType),
        };

        // Nothing further will ever happen to a NotApplied event -- of any
        // reason, including UnhandledType -- so it is marked processed even
        // though nothing was applied (decision 5). The Applied + sink case
        // is wired in by the next task; for now Applied is unreachable.
        self.webhook_events.mark_processed(event.id).await?;
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use domain::{WebhookEvent, WebhookEventId};
    use serde_json::json;
    use time::OffsetDateTime;
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
}
