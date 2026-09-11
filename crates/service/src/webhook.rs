use async_trait::async_trait;
use domain::{
    BillingEvent, BillingEventSink, CustomerRepository, DomainError, InvoiceRepository,
    PaymentMethodRepository, PlanRepository, SubscriptionRepository, VerifiedEvent,
    WebhookEventRepository,
};

use crate::{checkout_session, invoice_events, payment_method_events, subscription_lifecycle};

/// The outcome of processing one verified webhook event.
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
/// in `api`: collapsing them into `Ok(())` would make the
/// first production question -- "why isn't my subscription updating?" --
/// unanswerable from logs. `UnknownCustomer` in particular is not a failure:
/// it means the event concerns a Stripe customer this module does not own,
/// and a 5xx would make Stripe retry forever over data that will never
/// exist here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAppliedReason {
    /// Older than the last event applied to this row.
    Stale,
    /// No local customer row for the Stripe customer id.
    UnknownCustomer,
    /// No local mirror row for the Stripe subscription id.
    UnknownSubscription,
    /// No local mirror row for the Stripe payment-method id.
    UnknownPaymentMethod,
    /// No local plan mirrors the Stripe price the subscription is on, so its
    /// mirror cannot be created (`billing.subscriptions.plan_id` is
    /// `NOT NULL`). A verified event still never 5xxes.
    UnknownPlan,
    /// The event type is handled, but this delivery carried nothing to
    /// mirror -- e.g. `setup_intent.succeeded`, which only confirms a flow
    /// finished. Distinct from `UnhandledType`: recognised, not ignored.
    Acknowledged,
    /// No handler for this event type -- acknowledged, never rejected, so a
    /// new Stripe event type can never cause a failure.
    UnhandledType,
}

/// Object-safe handler for one verified, deduplicated webhook event.
///
/// `api`'s `AppState` holds this as `Arc<dyn WebhookHandler>` rather
/// than the generic [`WebhookProcessor`] directly, so the router factory
/// does not have to carry its port type parameters through its signature --
/// the same trade `BillingProvider` and `WebhookVerifier` made.
#[async_trait]
pub trait WebhookHandler: Send + Sync {
    /// Processes one verified event, reporting why nothing changed when
    /// nothing did.
    async fn handle(&self, event: VerifiedEvent) -> Result<EventOutcome, DomainError>;
}

/// Orchestrates webhook processing over the ports it needs: customer,
/// subscription and invoice lookups, the webhook ledger, and the host's
/// event sink.
///
/// Generic over every port, matching `StripeBillingProvider<R>`: the compiler
/// monomorphises the real wiring `demo` picks, and nothing is boxed on the
/// hot path. Implements the object-safe [`WebhookHandler`] below so `api`'s
/// `AppState` can hold it as `Arc<dyn WebhookHandler>` instead -- so
/// the parameter count stays between `demo` and `new`, never reaching `api`.
pub struct WebhookProcessor<C, S, I, P, L, W, K> {
    customers: C,
    subscriptions: S,
    invoices: I,
    payment_methods: P,
    plans: L,
    webhook_events: W,
    sink: K,
}

impl<C, S, I, P, L, W, K> WebhookProcessor<C, S, I, P, L, W, K> {
    /// Wraps the ports webhook processing needs.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        customers: C,
        subscriptions: S,
        invoices: I,
        payment_methods: P,
        plans: L,
        webhook_events: W,
        sink: K,
    ) -> Self {
        Self {
            customers,
            subscriptions,
            invoices,
            payment_methods,
            plans,
            webhook_events,
            sink,
        }
    }
}

#[async_trait]
impl<C, S, I, P, L, W, K> WebhookHandler for WebhookProcessor<C, S, I, P, L, W, K>
where
    C: CustomerRepository + Send + Sync,
    S: SubscriptionRepository + Send + Sync,
    I: InvoiceRepository + Send + Sync,
    P: PaymentMethodRepository + Send + Sync,
    L: PlanRepository + Send + Sync,
    W: WebhookEventRepository + Send + Sync,
    K: BillingEventSink,
{
    async fn handle(&self, event: VerifiedEvent) -> Result<EventOutcome, DomainError> {
        // No match on anything but the type string. An unrecognised type is
        // acknowledged, never rejected. The subscription lifecycle
        // events share one handler: same `data.object` shape, same
        // resolve-tenant -> mirror -> ordering-guard flow, `billing_event_for`
        // maps each status to the right `BillingEvent`. Only `created` may
        // create the mirror when it is absent (resolving `plan_id` from the
        // price); `updated`/`deleted` for an unknown subscription stay
        // `NotApplied`.
        let outcome = match event.event_type.as_str() {
            "customer.subscription.created" => {
                subscription_lifecycle::apply(
                    &self.customers,
                    &self.subscriptions,
                    &self.plans,
                    &event,
                    subscription_lifecycle::OnMissing::Create,
                )
                .await?
            }
            "customer.subscription.updated" | "customer.subscription.deleted" => {
                subscription_lifecycle::apply(
                    &self.customers,
                    &self.subscriptions,
                    &self.plans,
                    &event,
                    subscription_lifecycle::OnMissing::NotApplied,
                )
                .await?
            }
            // The two invoice events share a handler for the same reason:
            // one `data.object` shape, one resolve-tenant -> mirror flow,
            // differing only in the status written and the event emitted.
            "invoice.paid" => {
                invoice_events::apply(
                    &self.customers,
                    &self.subscriptions,
                    &self.invoices,
                    &event,
                    invoice_events::Kind::Paid,
                )
                .await?
            }
            "invoice.payment_failed" => {
                invoice_events::apply(
                    &self.customers,
                    &self.subscriptions,
                    &self.invoices,
                    &event,
                    invoice_events::Kind::PaymentFailed,
                )
                .await?
            }
            "payment_method.attached" => {
                payment_method_events::apply_attached(
                    &self.customers,
                    &self.payment_methods,
                    &event,
                )
                .await?
            }
            "payment_method.detached" => {
                payment_method_events::apply_detached(
                    &self.customers,
                    &self.payment_methods,
                    &event,
                )
                .await?
            }
            // Recognised, but the SetupIntent carries nothing to mirror --
            // the card details arrive via `payment_method.attached`.
            "setup_intent.succeeded" => EventOutcome::NotApplied(NotAppliedReason::Acknowledged),
            "checkout.session.completed" => {
                checkout_session::apply(&self.customers, &event).await?
            }
            _ => EventOutcome::NotApplied(NotAppliedReason::UnhandledType),
        };

        // The sink is a side effect: it runs after the mirror is a fact
        // (the match above already committed the write), and its failure
        // does not undo the mirror. It is never called for a
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
            // IS NULL find this event again.
            return Ok(outcome);
        }

        // Nothing further will ever happen to a NotApplied event -- of any
        // reason, including UnhandledType -- so it is marked processed even
        // though nothing was applied.
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
        InMemoryCustomers, InMemoryInvoices, InMemoryPaymentMethods, InMemoryPlans, InMemorySink,
        InMemorySubscriptions, InMemoryWebhookEvents,
    };

    /// Compiles only if `WebhookHandler` is dyn-compatible -- the property
    /// the `#[async_trait]` decision exists for.
    #[allow(dead_code)]
    fn assert_dyn_compatible(_handler: &dyn WebhookHandler) {}

    type TestProcessor = WebhookProcessor<
        InMemoryCustomers,
        InMemorySubscriptions,
        InMemoryInvoices,
        InMemoryPaymentMethods,
        InMemoryPlans,
        InMemoryWebhookEvents,
        InMemorySink,
    >;

    fn processor() -> TestProcessor {
        WebhookProcessor::new(
            InMemoryCustomers::default(),
            InMemorySubscriptions::default(),
            InMemoryInvoices::default(),
            InMemoryPaymentMethods::default(),
            InMemoryPlans::default(),
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

    /// Builds a `customer.subscription.*` payload with the shape
    /// `subscription_lifecycle::read_subscription` expects, for a given
    /// event type, Stripe customer/subscription id and status.
    fn lifecycle_payload(
        event_type: &str,
        stripe_customer_id: &str,
        stripe_subscription_id: &str,
        status: &str,
    ) -> Value {
        let now = OffsetDateTime::now_utc();
        json!({
            "id": stripe_subscription_id,
            "type": event_type,
            "data": {
                "object": {
                    "id": stripe_subscription_id,
                    "customer": stripe_customer_id,
                    "status": status,
                    "cancel_at_period_end": false,
                    "items": {
                        "data": [
                            {
                                "id": "si_test",
                                "price": { "id": "price_test" },
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
    /// `VerifiedEvent` of `event_type` carrying `status` that will apply
    /// cleanly against them. The seeded mirror starts `Incomplete` with no
    /// prior event, so the returned event is always admitted by the guard.
    fn seeded_lifecycle_event(
        processor: &TestProcessor,
        event_type: &str,
        status: &str,
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

        let payload = lifecycle_payload(event_type, "cus_applied", "sub_applied", status);
        let event = VerifiedEvent {
            id: WebhookEventId::new(Uuid::new_v4()),
            stripe_event_id: format!("evt_{}", Uuid::new_v4()),
            event_type: event_type.to_string(),
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

    /// The `customer.subscription.updated` / `active` case the pre-Task-16
    /// tests were written against.
    fn seeded_applied_event(
        processor: &TestProcessor,
    ) -> (TenantId, SubscriptionId, VerifiedEvent) {
        seeded_lifecycle_event(processor, "customer.subscription.updated", "active")
    }

    /// Builds a `VerifiedEvent` for the `cus_applied` / `sub_applied` pair
    /// `seeded_lifecycle_event` seeds, with an explicit `created` so a test
    /// can order two events against one already-seeded mirror.
    fn lifecycle_event(event_type: &str, status: &str, created: OffsetDateTime) -> VerifiedEvent {
        VerifiedEvent {
            id: WebhookEventId::new(Uuid::new_v4()),
            stripe_event_id: format!("evt_{}", Uuid::new_v4()),
            event_type: event_type.to_string(),
            created,
            payload: lifecycle_payload(event_type, "cus_applied", "sub_applied", status),
        }
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
            InMemoryInvoices::default(),
            InMemoryPaymentMethods::default(),
            InMemoryPlans::default(),
            InMemoryWebhookEvents::default(),
            InMemorySink::failing(),
        );
        let (tenant_id, subscription_id, event) = seeded_applied_event(&processor);

        let outcome = processor.handle(event.clone()).await;

        assert!(matches!(outcome, Ok(EventOutcome::Applied(_))));
        // The mirror write already committed before the sink was ever
        // called -- it is retained regardless of the sink's outcome.
        let found = processor
            .subscriptions
            .find(tenant_id, subscription_id)
            .await;
        assert!(matches!(
            found,
            Ok(Some(ref s)) if s.status == domain::SubscriptionStatus::Active
        ));
        // processing did not finish, so processed_at stays NULL -- this is
        // what makes a later recovery sweep able to find it.
        assert_eq!(processor.webhook_events.processed_at(event.id), Some(None));
    }

    // --- customer.subscription.created / .deleted ---

    #[tokio::test]
    async fn created_event_routes_through_and_applies() {
        let processor = processor();
        let (tenant_id, subscription_id, event) =
            seeded_lifecycle_event(&processor, "customer.subscription.created", "active");

        let outcome = processor.handle(event.clone()).await;

        assert_eq!(
            outcome,
            Ok(EventOutcome::Applied(BillingEvent::SubscriptionActivated {
                tenant_id,
                subscription_id,
            }))
        );
        assert_eq!(processor.sink.received().len(), 1);
        assert!(matches!(
            processor.webhook_events.processed_at(event.id),
            Some(Some(_))
        ));
    }

    #[tokio::test]
    async fn deleted_event_applies_and_emits_subscription_canceled() {
        let processor = processor();
        let (tenant_id, subscription_id, event) =
            seeded_lifecycle_event(&processor, "customer.subscription.deleted", "canceled");

        let outcome = processor.handle(event).await;

        assert_eq!(
            outcome,
            Ok(EventOutcome::Applied(BillingEvent::SubscriptionCanceled {
                tenant_id,
                subscription_id,
            }))
        );
        let found = processor
            .subscriptions
            .find(tenant_id, subscription_id)
            .await;
        assert!(matches!(
            found,
            Ok(Some(ref s)) if s.status == domain::SubscriptionStatus::Canceled
        ));
    }

    #[tokio::test]
    async fn created_for_unknown_subscription_with_no_local_plan_is_unknown_plan() {
        let processor = processor();
        let tenant_id = TenantId::new(Uuid::new_v4());
        processor.customers.seed(Customer {
            id: CustomerId::new(Uuid::new_v4()),
            tenant_id,
            stripe_customer_id: Some("cus_lonely".to_string()),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        });
        // No plan seeded for `price_test`.
        let event = VerifiedEvent {
            id: WebhookEventId::new(Uuid::new_v4()),
            stripe_event_id: format!("evt_{}", Uuid::new_v4()),
            event_type: "customer.subscription.created".to_string(),
            created: OffsetDateTime::now_utc(),
            payload: lifecycle_payload(
                "customer.subscription.created",
                "cus_lonely",
                "sub_never_mirrored",
                "active",
            ),
        };

        let outcome = processor.handle(event).await;

        assert_eq!(
            outcome,
            Ok(EventOutcome::NotApplied(NotAppliedReason::UnknownPlan))
        );
        // No honest row to write without a plan_id.
        let listed = processor.subscriptions.list(tenant_id).await;
        assert!(matches!(listed, Ok(ref rows) if rows.is_empty()));
        assert_eq!(processor.sink.received().len(), 0);
    }

    #[tokio::test]
    async fn created_for_unknown_subscription_creates_the_mirror_when_the_plan_is_known() {
        let processor = processor();
        let tenant_id = TenantId::new(Uuid::new_v4());
        processor.customers.seed(Customer {
            id: CustomerId::new(Uuid::new_v4()),
            tenant_id,
            stripe_customer_id: Some("cus_boot".to_string()),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        });
        processor.plans.seed(domain::Plan {
            id: domain::PlanId::new(Uuid::new_v4()),
            tenant_id,
            stripe_price_id: "price_test".to_string(),
            stripe_product_id: "prod_test".to_string(),
            name: "Boot".to_string(),
            amount: domain::Money::new(1500, domain::Currency::Usd),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        });
        let event = VerifiedEvent {
            id: WebhookEventId::new(Uuid::new_v4()),
            stripe_event_id: format!("evt_{}", Uuid::new_v4()),
            event_type: "customer.subscription.created".to_string(),
            created: OffsetDateTime::now_utc(),
            payload: lifecycle_payload(
                "customer.subscription.created",
                "cus_boot",
                "sub_bootstrapped",
                "active",
            ),
        };

        let outcome = processor.handle(event).await;

        assert!(matches!(
            outcome,
            Ok(EventOutcome::Applied(BillingEvent::SubscriptionActivated { tenant_id: t, .. })) if t == tenant_id
        ));
        let rows = processor.subscriptions.list(tenant_id).await;
        assert!(matches!(
            rows,
            Ok(ref v) if v.len() == 1
                && v[0].stripe_subscription_id == "sub_bootstrapped"
                && v[0].status == domain::SubscriptionStatus::Active
                && v[0].last_event_created_at.is_some()
        ));
        assert_eq!(processor.sink.received().len(), 1);
    }

    #[tokio::test]
    async fn deleted_for_unknown_customer_is_not_applied() {
        let processor = processor();
        let event = VerifiedEvent {
            id: WebhookEventId::new(Uuid::new_v4()),
            stripe_event_id: format!("evt_{}", Uuid::new_v4()),
            event_type: "customer.subscription.deleted".to_string(),
            created: OffsetDateTime::now_utc(),
            payload: lifecycle_payload(
                "customer.subscription.deleted",
                "cus_unknown",
                "sub_unknown",
                "canceled",
            ),
        };

        let outcome = processor.handle(event).await;

        assert_eq!(
            outcome,
            Ok(EventOutcome::NotApplied(NotAppliedReason::UnknownCustomer))
        );
    }

    #[tokio::test]
    async fn stale_deleted_cannot_cancel_a_row_a_newer_update_reactivated() {
        let processor = processor();
        let (tenant_id, subscription_id, newer_update) =
            seeded_lifecycle_event(&processor, "customer.subscription.updated", "active");

        let applied = processor.handle(newer_update.clone()).await;
        assert!(matches!(applied, Ok(EventOutcome::Applied(_))));

        // A `deleted` that Stripe created *before* the update above -- e.g. a
        // delayed redelivery. The guard must reject it on `created`, not
        // status.
        let stale_delete = lifecycle_event(
            "customer.subscription.deleted",
            "canceled",
            newer_update.created - Duration::minutes(5),
        );

        let outcome = processor.handle(stale_delete).await;

        assert_eq!(
            outcome,
            Ok(EventOutcome::NotApplied(NotAppliedReason::Stale))
        );
        let found = processor
            .subscriptions
            .find(tenant_id, subscription_id)
            .await;
        assert!(matches!(
            found,
            Ok(Some(ref s)) if s.status == domain::SubscriptionStatus::Active
        ));
    }

    // --- invoice.paid / invoice.payment_failed ---

    /// Seeds a customer and returns its tenant id plus a `VerifiedEvent` of
    /// `event_type` for an invoice `in_applied` against it.
    fn seeded_invoice_event(
        processor: &TestProcessor,
        event_type: &str,
    ) -> (TenantId, VerifiedEvent) {
        let tenant_id = TenantId::new(Uuid::new_v4());
        processor.customers.seed(Customer {
            id: CustomerId::new(Uuid::new_v4()),
            tenant_id,
            stripe_customer_id: Some("cus_inv".to_string()),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        });
        let event = VerifiedEvent {
            id: WebhookEventId::new(Uuid::new_v4()),
            stripe_event_id: format!("evt_{}", Uuid::new_v4()),
            event_type: event_type.to_string(),
            created: OffsetDateTime::now_utc(),
            payload: json!({
                "id": "evt_inv",
                "type": event_type,
                "data": { "object": {
                    "id": "in_applied",
                    "customer": "cus_inv",
                    "total": 4200,
                    "currency": "usd",
                }},
            }),
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
        (tenant_id, event)
    }

    #[tokio::test]
    async fn invoice_paid_routes_through_handle_and_notifies_the_sink_once() {
        let processor = processor();
        let (tenant_id, event) = seeded_invoice_event(&processor, "invoice.paid");

        let outcome = processor.handle(event.clone()).await;

        assert!(matches!(
            outcome,
            Ok(EventOutcome::Applied(BillingEvent::PaymentSucceeded { tenant_id: t, .. })) if t == tenant_id
        ));
        assert_eq!(processor.sink.received().len(), 1);
        assert!(matches!(
            processor.webhook_events.processed_at(event.id),
            Some(Some(_))
        ));
    }

    #[tokio::test]
    async fn invoice_payment_failed_routes_and_emits_payment_failed() {
        let processor = processor();
        let (tenant_id, event) = seeded_invoice_event(&processor, "invoice.payment_failed");

        let outcome = processor.handle(event).await;

        assert!(matches!(
            outcome,
            Ok(EventOutcome::Applied(BillingEvent::PaymentFailed { tenant_id: t, .. })) if t == tenant_id
        ));
        assert_eq!(processor.sink.received().len(), 1);
    }

    // --- payment_method.* / setup_intent.succeeded ---

    #[tokio::test]
    async fn payment_method_attached_routes_through_handle_and_notifies_the_sink_once() {
        let processor = processor();
        let tenant_id = TenantId::new(Uuid::new_v4());
        processor.customers.seed(Customer {
            id: CustomerId::new(Uuid::new_v4()),
            tenant_id,
            stripe_customer_id: Some("cus_pm".to_string()),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        });
        let event = VerifiedEvent {
            id: WebhookEventId::new(Uuid::new_v4()),
            stripe_event_id: format!("evt_{}", Uuid::new_v4()),
            event_type: "payment_method.attached".to_string(),
            created: OffsetDateTime::now_utc(),
            payload: json!({
                "id": "evt_pm",
                "type": "payment_method.attached",
                "data": { "object": {
                    "id": "pm_routed",
                    "type": "card",
                    "customer": "cus_pm",
                    "card": { "brand": "visa", "last4": "4242" },
                }},
            }),
        };

        let outcome = processor.handle(event).await;

        assert!(matches!(
            outcome,
            Ok(EventOutcome::Applied(BillingEvent::PaymentMethodAttached { tenant_id: t, .. })) if t == tenant_id
        ));
        assert_eq!(processor.sink.received().len(), 1);
    }

    #[tokio::test]
    async fn setup_intent_succeeded_is_acknowledged_and_marked_processed() {
        let processor = processor();
        let event = verified_event("setup_intent.succeeded");
        processor.webhook_events.seed(WebhookEvent {
            id: event.id,
            tenant_id: None,
            stripe_event_id: event.stripe_event_id.clone(),
            event_type: event.event_type.clone(),
            payload: event.payload.clone(),
            created_at: event.created,
            processed_at: None,
        });

        let outcome = processor.handle(event.clone()).await;

        assert_eq!(
            outcome,
            Ok(EventOutcome::NotApplied(NotAppliedReason::Acknowledged))
        );
        assert_eq!(processor.sink.received().len(), 0);
        assert!(matches!(
            processor.webhook_events.processed_at(event.id),
            Some(Some(_))
        ));
    }
}
