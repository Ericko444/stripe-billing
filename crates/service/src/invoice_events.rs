use domain::{
    BillingEvent, Currency, CustomerRepository, DomainError, EventApplication, InvoiceId,
    InvoiceRepository, InvoiceStatus, Money, SubscriptionRepository, TenantId, VerifiedEvent,
};
use serde_json::Value;

use crate::webhook::{EventOutcome, NotAppliedReason};

/// Which invoice event is being applied. Each of the two §10.4 invoice
/// types pins both the status mirrored onto the row and the `BillingEvent`
/// handed to the sink, so there is no partial/unreachable state to handle.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    /// `invoice.paid`.
    Paid,
    /// `invoice.payment_failed`.
    PaymentFailed,
}

impl Kind {
    fn status(self) -> InvoiceStatus {
        match self {
            Kind::Paid => InvoiceStatus::Paid,
            Kind::PaymentFailed => InvoiceStatus::Failed,
        }
    }
}

/// Applies an `invoice.paid` / `invoice.payment_failed` event: resolves the
/// tenant from the invoice's Stripe customer id (§10.3), best-effort links
/// the local subscription mirror, and mirrors the invoice through
/// [`InvoiceRepository::apply_event`]'s upsert + ordering guard (§10.2).
///
/// Unlike the subscription handler this may *create* the local invoice row:
/// there is no `invoice.created` webhook, so `paid` / `payment_failed` are
/// the first the module sees. Tenant still comes from the database, never
/// the payload -- `data.object.customer` is Stripe's own id, looked up here.
pub async fn apply<C, S, I>(
    customers: &C,
    subscriptions: &S,
    invoices: &I,
    event: &VerifiedEvent,
    kind: Kind,
) -> Result<EventOutcome, DomainError>
where
    C: CustomerRepository,
    S: SubscriptionRepository,
    I: InvoiceRepository,
{
    let fields = read_invoice(&event.payload)?;

    let Some(customer) = customers
        .find_by_stripe_customer_id(&fields.stripe_customer_id)
        .await?
    else {
        return Ok(EventOutcome::NotApplied(NotAppliedReason::UnknownCustomer));
    };
    let tenant_id = customer.tenant_id;

    // Best-effort: an invoice can name a subscription whose mirror row this
    // module does not have. That is not a failure -- the invoice still
    // mirrors, with a NULL subscription link.
    let subscription_id = match fields.stripe_subscription_id {
        Some(ref stripe_sub_id) => subscriptions
            .find_by_stripe_subscription_id(tenant_id, stripe_sub_id)
            .await?
            .map(|s| s.id),
        None => None,
    };

    let application = invoices
        .apply_event(
            tenant_id,
            customer.id,
            subscription_id,
            &fields.stripe_invoice_id,
            fields.amount,
            kind.status(),
            event.created,
        )
        .await?;

    if application == EventApplication::Stale {
        return Ok(EventOutcome::NotApplied(NotAppliedReason::Stale));
    }

    // Read the mirrored row back for its local id -- `apply_event` is an
    // upsert and does not return one, and the row's own `subscription_id` is
    // the authoritative link to put in the notification.
    let mirrored = invoices
        .find_by_stripe_invoice_id(tenant_id, &fields.stripe_invoice_id)
        .await?
        .ok_or_else(|| {
            DomainError::Repository("invoice missing immediately after apply_event".to_string())
        })?;

    Ok(EventOutcome::Applied(billing_event(
        kind,
        tenant_id,
        mirrored.id,
        mirrored.subscription_id,
    )))
}

fn billing_event(
    kind: Kind,
    tenant_id: TenantId,
    invoice_id: InvoiceId,
    subscription_id: Option<domain::SubscriptionId>,
) -> BillingEvent {
    match kind {
        Kind::Paid => BillingEvent::PaymentSucceeded {
            tenant_id,
            invoice_id,
            subscription_id,
        },
        Kind::PaymentFailed => BillingEvent::PaymentFailed {
            tenant_id,
            invoice_id,
            subscription_id,
        },
    }
}

/// The fields this handler needs off an `invoice.*` envelope's `data.object`.
struct InvoiceFields {
    stripe_customer_id: String,
    stripe_invoice_id: String,
    stripe_subscription_id: Option<String>,
    amount: Money,
}

/// Reads the fields from the raw payload's `data.object` through `Value`
/// accessors, the same no-`serde`-derive approach as
/// `subscription_lifecycle::read_subscription`. A missing or wrong-typed
/// required field is a [`DomainError::MalformedEvent`], never an index panic.
///
/// `subscription` is read from `data.object.subscription` and, failing that,
/// `data.object.parent.subscription_details.subscription` -- recent Stripe
/// API versions moved the link under `parent`. Absent from both is `None`,
/// not an error: not every invoice is a subscription invoice.
fn read_invoice(payload: &Value) -> Result<InvoiceFields, DomainError> {
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
                DomainError::MalformedEvent(format!("invoice object missing string `{name}`"))
            })
    };

    let stripe_customer_id = field_str("customer")?;
    let stripe_invoice_id = field_str("id")?;

    let stripe_subscription_id = object
        .get("subscription")
        .and_then(Value::as_str)
        .or_else(|| {
            object
                .get("parent")
                .and_then(|p| p.get("subscription_details"))
                .and_then(|d| d.get("subscription"))
                .and_then(Value::as_str)
        })
        .map(str::to_string);

    let total = object.get("total").and_then(Value::as_i64).ok_or_else(|| {
        DomainError::MalformedEvent("invoice object missing integer `total`".to_string())
    })?;
    let currency = currency_from_code(&field_str("currency")?)?;

    Ok(InvoiceFields {
        stripe_customer_id,
        stripe_invoice_id,
        stripe_subscription_id,
        amount: Money::new(total, currency),
    })
}

/// Maps Stripe's lowercase ISO currency code to the domain [`Currency`].
/// `service` cannot reach `persistence`'s codec, and only the billable set
/// is supported -- an invoice in any other currency is a
/// [`DomainError::MalformedEvent`] rather than a silent default.
fn currency_from_code(code: &str) -> Result<Currency, DomainError> {
    match code.to_ascii_lowercase().as_str() {
        "usd" => Ok(Currency::Usd),
        "eur" => Ok(Currency::Eur),
        "gbp" => Ok(Currency::Gbp),
        other => Err(DomainError::MalformedEvent(format!(
            "unsupported invoice currency: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use domain::{
        Customer, CustomerId, Invoice, PlanId, Subscription, SubscriptionId, SubscriptionStatus,
        WebhookEventId,
    };
    use serde_json::json;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{InMemoryCustomers, InMemoryInvoices, InMemorySubscriptions};

    struct Ports {
        customers: InMemoryCustomers,
        subscriptions: InMemorySubscriptions,
        invoices: InMemoryInvoices,
    }

    impl Ports {
        fn new() -> Self {
            Self {
                customers: InMemoryCustomers::default(),
                subscriptions: InMemorySubscriptions::default(),
                invoices: InMemoryInvoices::default(),
            }
        }

        async fn apply(
            &self,
            event: &VerifiedEvent,
            kind: Kind,
        ) -> Result<EventOutcome, DomainError> {
            apply(
                &self.customers,
                &self.subscriptions,
                &self.invoices,
                event,
                kind,
            )
            .await
        }

        /// Seeds a customer and returns its tenant + local ids.
        fn seed_customer(&self, stripe_customer_id: &str) -> (TenantId, CustomerId) {
            let tenant_id = TenantId::new(Uuid::new_v4());
            let customer_id = CustomerId::new(Uuid::new_v4());
            self.customers.seed(Customer {
                id: customer_id,
                tenant_id,
                stripe_customer_id: Some(stripe_customer_id.to_string()),
                created_at: OffsetDateTime::now_utc(),
                deleted_at: None,
            });
            (tenant_id, customer_id)
        }
    }

    fn invoice_payload(
        event_type: &str,
        stripe_customer_id: &str,
        stripe_invoice_id: &str,
        stripe_subscription_id: Option<&str>,
        total: i64,
        currency: &str,
    ) -> Value {
        let mut object = json!({
            "id": stripe_invoice_id,
            "customer": stripe_customer_id,
            "total": total,
            "currency": currency,
        });
        if let Some(sub) = stripe_subscription_id {
            object["subscription"] = json!(sub);
        }
        json!({ "id": "evt_test", "type": event_type, "data": { "object": object } })
    }

    fn verified(payload: Value, created: OffsetDateTime) -> VerifiedEvent {
        VerifiedEvent {
            id: WebhookEventId::new(Uuid::new_v4()),
            stripe_event_id: format!("evt_{}", Uuid::new_v4()),
            event_type: "invoice.paid".to_string(),
            created,
            payload,
        }
    }

    #[tokio::test]
    async fn invoice_paid_mirrors_and_emits_payment_succeeded() -> Result<(), Box<dyn Error>> {
        let ports = Ports::new();
        let (tenant_id, _) = ports.seed_customer("cus_1");
        let event = verified(
            invoice_payload("invoice.paid", "cus_1", "in_1", None, 4200, "usd"),
            OffsetDateTime::now_utc(),
        );

        let outcome = ports.apply(&event, Kind::Paid).await?;

        let mirrored = ports
            .invoices
            .find_by_stripe_invoice_id(tenant_id, "in_1")
            .await?
            .ok_or("row mirrored")?;
        assert_eq!(
            outcome,
            EventOutcome::Applied(BillingEvent::PaymentSucceeded {
                tenant_id,
                invoice_id: mirrored.id,
                subscription_id: None,
            })
        );
        assert_eq!(mirrored.status, InvoiceStatus::Paid);
        assert_eq!(mirrored.amount, Money::new(4200, Currency::Usd));
        Ok(())
    }

    #[tokio::test]
    async fn invoice_payment_failed_emits_payment_failed_once() -> Result<(), Box<dyn Error>> {
        let ports = Ports::new();
        let (tenant_id, _) = ports.seed_customer("cus_2");
        let event = verified(
            invoice_payload("invoice.payment_failed", "cus_2", "in_2", None, 900, "eur"),
            OffsetDateTime::now_utc(),
        );

        let outcome = ports.apply(&event, Kind::PaymentFailed).await?;

        let mirrored = ports
            .invoices
            .find_by_stripe_invoice_id(tenant_id, "in_2")
            .await?
            .ok_or("row mirrored")?;
        assert_eq!(
            outcome,
            EventOutcome::Applied(BillingEvent::PaymentFailed {
                tenant_id,
                invoice_id: mirrored.id,
                subscription_id: None,
            })
        );
        assert_eq!(mirrored.status, InvoiceStatus::Failed);
        Ok(())
    }

    #[tokio::test]
    async fn unknown_customer_is_not_applied_and_mirrors_nothing() -> Result<(), Box<dyn Error>> {
        let ports = Ports::new();
        let event = verified(
            invoice_payload("invoice.paid", "cus_absent", "in_x", None, 100, "usd"),
            OffsetDateTime::now_utc(),
        );

        let outcome = ports.apply(&event, Kind::Paid).await?;

        assert_eq!(
            outcome,
            EventOutcome::NotApplied(NotAppliedReason::UnknownCustomer)
        );
        assert_eq!(
            ports.invoices.list(TenantId::new(Uuid::new_v4())).await?,
            Vec::new()
        );
        Ok(())
    }

    #[tokio::test]
    async fn known_subscription_is_linked_on_the_mirror() -> Result<(), Box<dyn Error>> {
        let ports = Ports::new();
        let (tenant_id, customer_id) = ports.seed_customer("cus_3");

        let now = OffsetDateTime::now_utc();
        let subscription = Subscription {
            id: SubscriptionId::new(Uuid::new_v4()),
            tenant_id,
            customer_id,
            plan_id: PlanId::new(Uuid::new_v4()),
            stripe_subscription_id: "sub_linked".to_string(),
            stripe_subscription_item_id: "si_x".to_string(),
            status: SubscriptionStatus::Active,
            current_period_start: now,
            current_period_end: now + Duration::days(30),
            cancel_at_period_end: false,
            last_event_created_at: None,
            created_at: now,
            deleted_at: None,
        };
        ports.subscriptions.seed(subscription.clone());

        let event = verified(
            invoice_payload(
                "invoice.paid",
                "cus_3",
                "in_3",
                Some("sub_linked"),
                4200,
                "usd",
            ),
            now,
        );

        let outcome = ports.apply(&event, Kind::Paid).await?;

        assert!(matches!(
            outcome,
            EventOutcome::Applied(BillingEvent::PaymentSucceeded {
                subscription_id: Some(id),
                ..
            }) if id == subscription.id
        ));
        let mirrored = ports
            .invoices
            .find_by_stripe_invoice_id(tenant_id, "in_3")
            .await?
            .ok_or("row mirrored")?;
        assert_eq!(mirrored.subscription_id, Some(subscription.id));
        Ok(())
    }

    #[tokio::test]
    async fn stale_invoice_event_does_not_regress_the_row() -> Result<(), Box<dyn Error>> {
        let ports = Ports::new();
        let (tenant_id, customer_id) = ports.seed_customer("cus_4");

        // A row already carrying a newer event, seeded directly.
        let newer = OffsetDateTime::now_utc();
        ports.invoices.seed(Invoice {
            id: InvoiceId::new(Uuid::new_v4()),
            tenant_id,
            customer_id,
            subscription_id: None,
            stripe_invoice_id: "in_4".to_string(),
            amount: Money::new(4200, Currency::Usd),
            status: InvoiceStatus::Paid,
            last_event_created_at: Some(newer),
            created_at: newer,
            deleted_at: None,
        });

        let older_event = verified(
            invoice_payload("invoice.payment_failed", "cus_4", "in_4", None, 4200, "usd"),
            newer - Duration::minutes(5),
        );

        let outcome = ports.apply(&older_event, Kind::PaymentFailed).await?;

        assert_eq!(outcome, EventOutcome::NotApplied(NotAppliedReason::Stale));
        let mirrored = ports
            .invoices
            .find_by_stripe_invoice_id(tenant_id, "in_4")
            .await?
            .ok_or("row exists")?;
        assert_eq!(mirrored.status, InvoiceStatus::Paid);
        Ok(())
    }

    #[tokio::test]
    async fn a_payload_missing_total_is_a_typed_error_not_a_panic() -> Result<(), Box<dyn Error>> {
        let ports = Ports::new();
        ports.seed_customer("cus_5");

        let mut payload = invoice_payload("invoice.paid", "cus_5", "in_5", None, 0, "usd");
        payload["data"]["object"]
            .as_object_mut()
            .ok_or("object")?
            .remove("total");
        let event = verified(payload, OffsetDateTime::now_utc());

        let outcome = ports.apply(&event, Kind::Paid).await;

        assert!(matches!(outcome, Err(DomainError::MalformedEvent(_))));
        Ok(())
    }
}
