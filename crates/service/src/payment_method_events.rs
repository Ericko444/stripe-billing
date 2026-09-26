use audit::{Action, Actor, AuditEntry, CorrelationId, Target, TargetId};
use domain::{
    BillingEvent, CustomerRepository, DomainError, EventApplication, PaymentMethodRepository,
    VerifiedEvent,
};
use serde_json::Value;
use uuid::Uuid;

use crate::webhook::{EventOutcome, NotAppliedReason};

/// Applies `payment_method.attached`: resolves the tenant from
/// `data.object.customer`, then mirrors the card through
/// [`PaymentMethodRepository::apply_event`]'s upsert + ordering guard. Like
/// the invoice handler this may *create* the local row -- `attached` is the
/// first event the module sees for a card.
pub async fn apply_attached<C, P>(
    customers: &C,
    payment_methods: &P,
    event: &VerifiedEvent,
    correlation_id: Uuid,
) -> Result<EventOutcome, DomainError>
where
    C: CustomerRepository,
    P: PaymentMethodRepository,
{
    let fields = read_payment_method(&event.payload)?;
    let Some(stripe_customer_id) = fields.stripe_customer_id else {
        return Err(DomainError::MalformedEvent(
            "payment_method.attached object missing string `customer`".to_string(),
        ));
    };

    let Some(customer) = customers
        .find_by_stripe_customer_id(&stripe_customer_id)
        .await?
    else {
        return Ok(EventOutcome::NotApplied(NotAppliedReason::UnknownCustomer));
    };

    // `Target::Customer`, not `Target::PaymentMethod`: like the invoice
    // handler, this is an upsert and the local `PaymentMethodId` does not
    // exist yet when this call is made -- unlike `apply_detached` below,
    // which always looks the row up first.
    let entry = AuditEntry::new(
        audit::TenantId::new(customer.tenant_id.as_uuid()),
        Actor::System,
        Action::PaymentMethodAttached,
        Target::Customer(TargetId::new(customer.id.as_uuid())),
        event.created,
        CorrelationId::new(correlation_id),
    );
    let application = payment_methods
        .apply_event(
            customer.tenant_id,
            customer.id,
            &fields.stripe_payment_method_id,
            &fields.brand,
            &fields.last4,
            false,
            event.created,
            entry,
        )
        .await?;

    if application == EventApplication::Stale {
        return Ok(EventOutcome::NotApplied(NotAppliedReason::Stale));
    }

    let mirrored = payment_methods
        .find_by_stripe_payment_method_id(customer.tenant_id, &fields.stripe_payment_method_id)
        .await?
        .ok_or_else(|| {
            DomainError::Repository(
                "payment method missing immediately after apply_event".to_string(),
            )
        })?;

    Ok(EventOutcome::Applied(BillingEvent::PaymentMethodAttached {
        tenant_id: customer.tenant_id,
        payment_method_id: mirrored.id,
    }))
}

/// Applies `payment_method.detached`: a soft removal of the mirror row.
///
/// The detached object's `customer` is already `null`, so the tenant is
/// resolved from `data.previous_attributes.customer` -- the value the field
/// held before Stripe cleared it. An unknown card (never mirrored) is
/// `NotApplied(UnknownPaymentMethod)`, not an error.
pub async fn apply_detached<C, P>(
    customers: &C,
    payment_methods: &P,
    event: &VerifiedEvent,
    correlation_id: Uuid,
) -> Result<EventOutcome, DomainError>
where
    C: CustomerRepository,
    P: PaymentMethodRepository,
{
    let fields = read_payment_method(&event.payload)?;
    let Some(stripe_customer_id) = fields.stripe_customer_id else {
        return Err(DomainError::MalformedEvent(
            "payment_method.detached carried no customer on the object or in \
             previous_attributes"
                .to_string(),
        ));
    };

    let Some(customer) = customers
        .find_by_stripe_customer_id(&stripe_customer_id)
        .await?
    else {
        return Ok(EventOutcome::NotApplied(NotAppliedReason::UnknownCustomer));
    };

    let Some(mirrored) = payment_methods
        .find_by_stripe_payment_method_id(customer.tenant_id, &fields.stripe_payment_method_id)
        .await?
    else {
        return Ok(EventOutcome::NotApplied(
            NotAppliedReason::UnknownPaymentMethod,
        ));
    };

    // `mirrored` was already looked up above, so unlike `apply_attached`
    // the local `PaymentMethodId` is known here -- the target is the
    // payment method itself, not its customer.
    let entry = AuditEntry::new(
        audit::TenantId::new(customer.tenant_id.as_uuid()),
        Actor::System,
        Action::PaymentMethodDetached,
        Target::PaymentMethod(TargetId::new(mirrored.id.as_uuid())),
        event.created,
        CorrelationId::new(correlation_id),
    );
    let application = payment_methods
        .detach_event(
            customer.tenant_id,
            &fields.stripe_payment_method_id,
            event.created,
            entry,
        )
        .await?;

    Ok(match application {
        EventApplication::Applied => EventOutcome::Applied(BillingEvent::PaymentMethodDetached {
            tenant_id: customer.tenant_id,
            payment_method_id: mirrored.id,
        }),
        EventApplication::Stale => EventOutcome::NotApplied(NotAppliedReason::Stale),
    })
}

/// The fields the payment-method handlers need off a `payment_method.*`
/// envelope. `stripe_customer_id` is `data.object.customer`, or
/// `data.previous_attributes.customer` when the object's own is `null`
/// (the `detached` case). `brand` / `last4` come from `data.object.card`,
/// falling back to `data.object.type` for a non-card method.
struct PaymentMethodFields {
    stripe_customer_id: Option<String>,
    stripe_payment_method_id: String,
    brand: String,
    last4: String,
}

fn read_payment_method(payload: &Value) -> Result<PaymentMethodFields, DomainError> {
    let data = payload
        .get("data")
        .ok_or_else(|| DomainError::MalformedEvent("event envelope missing `data`".to_string()))?;
    let object = data.get("object").ok_or_else(|| {
        DomainError::MalformedEvent("event envelope missing data.object".to_string())
    })?;

    let stripe_payment_method_id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            DomainError::MalformedEvent("payment method object missing string `id`".to_string())
        })?;

    let stripe_customer_id = object
        .get("customer")
        .and_then(Value::as_str)
        .or_else(|| {
            data.get("previous_attributes")
                .and_then(|prev| prev.get("customer"))
                .and_then(Value::as_str)
        })
        .map(str::to_string);

    let card = object.get("card");
    let brand = card
        .and_then(|c| c.get("brand"))
        .and_then(Value::as_str)
        .or_else(|| object.get("type").and_then(Value::as_str))
        .unwrap_or("unknown")
        .to_string();
    let last4 = card
        .and_then(|c| c.get("last4"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    Ok(PaymentMethodFields {
        stripe_customer_id,
        stripe_payment_method_id,
        brand,
        last4,
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use domain::{Customer, CustomerId, PaymentMethod, PaymentMethodId, TenantId, WebhookEventId};
    use serde_json::json;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{InMemoryCustomers, InMemoryPaymentMethods};

    struct Ports {
        customers: InMemoryCustomers,
        payment_methods: InMemoryPaymentMethods,
    }

    impl Ports {
        fn new() -> Self {
            Self {
                customers: InMemoryCustomers::default(),
                payment_methods: InMemoryPaymentMethods::default(),
            }
        }

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

    fn attached_event(
        stripe_customer_id: &str,
        stripe_pm_id: &str,
        created: OffsetDateTime,
    ) -> VerifiedEvent {
        event(
            "payment_method.attached",
            json!({
                "id": stripe_pm_id,
                "type": "card",
                "customer": stripe_customer_id,
                "card": { "brand": "visa", "last4": "4242" },
            }),
            None,
            created,
        )
    }

    fn detached_event(
        prev_customer: &str,
        stripe_pm_id: &str,
        created: OffsetDateTime,
    ) -> VerifiedEvent {
        event(
            "payment_method.detached",
            json!({
                "id": stripe_pm_id,
                "type": "card",
                "customer": Value::Null,
                "card": { "brand": "visa", "last4": "4242" },
            }),
            Some(json!({ "customer": prev_customer })),
            created,
        )
    }

    fn event(
        event_type: &str,
        object: Value,
        previous_attributes: Option<Value>,
        created: OffsetDateTime,
    ) -> VerifiedEvent {
        let mut data = json!({ "object": object });
        if let Some(prev) = previous_attributes {
            data["previous_attributes"] = prev;
        }
        VerifiedEvent {
            id: WebhookEventId::new(Uuid::new_v4()),
            stripe_event_id: format!("evt_{}", Uuid::new_v4()),
            event_type: event_type.to_string(),
            created,
            payload: json!({ "id": "evt_test", "type": event_type, "data": data }),
        }
    }

    #[tokio::test]
    async fn attached_mirrors_the_card_and_emits_attached() -> Result<(), Box<dyn Error>> {
        let ports = Ports::new();
        let (tenant_id, customer_id) = ports.seed_customer("cus_1");
        let ev = attached_event("cus_1", "pm_1", OffsetDateTime::now_utc());

        let outcome = apply_attached(
            &ports.customers,
            &ports.payment_methods,
            &ev,
            Uuid::new_v4(),
        )
        .await?;

        let mirrored = ports
            .payment_methods
            .find_by_stripe_payment_method_id(tenant_id, "pm_1")
            .await?
            .ok_or("mirrored")?;
        assert_eq!(
            outcome,
            EventOutcome::Applied(BillingEvent::PaymentMethodAttached {
                tenant_id,
                payment_method_id: mirrored.id,
            })
        );
        assert_eq!(mirrored.brand, "visa");
        assert_eq!(mirrored.last4, "4242");

        let entries = ports.payment_methods.apply_event_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].actor(), audit::Actor::System);
        assert_eq!(entries[0].action(), audit::Action::PaymentMethodAttached);
        assert_eq!(
            entries[0].target(),
            audit::Target::Customer(audit::TargetId::new(customer_id.as_uuid()))
        );
        Ok(())
    }

    #[tokio::test]
    async fn attached_for_unknown_customer_is_not_applied() -> Result<(), Box<dyn Error>> {
        let ports = Ports::new();
        let ev = attached_event("cus_absent", "pm_x", OffsetDateTime::now_utc());

        let outcome = apply_attached(
            &ports.customers,
            &ports.payment_methods,
            &ev,
            Uuid::new_v4(),
        )
        .await?;

        assert_eq!(
            outcome,
            EventOutcome::NotApplied(NotAppliedReason::UnknownCustomer)
        );
        Ok(())
    }

    #[tokio::test]
    async fn detached_soft_removes_the_row_and_emits_detached() -> Result<(), Box<dyn Error>> {
        let ports = Ports::new();
        let (tenant_id, customer_id) = ports.seed_customer("cus_2");
        let attached_at = OffsetDateTime::now_utc();
        ports.payment_methods.seed(PaymentMethod {
            id: PaymentMethodId::new(Uuid::new_v4()),
            tenant_id,
            customer_id,
            stripe_payment_method_id: "pm_2".to_string(),
            brand: "visa".to_string(),
            last4: "4242".to_string(),
            is_default: false,
            last_event_created_at: Some(attached_at),
            created_at: attached_at,
            deleted_at: None,
        });

        let ev = detached_event("cus_2", "pm_2", attached_at + Duration::minutes(1));
        let outcome = apply_detached(
            &ports.customers,
            &ports.payment_methods,
            &ev,
            Uuid::new_v4(),
        )
        .await?;

        assert!(matches!(
            outcome,
            EventOutcome::Applied(BillingEvent::PaymentMethodDetached { tenant_id: t, .. }) if t == tenant_id
        ));
        assert_eq!(
            ports
                .payment_methods
                .find_by_stripe_payment_method_id(tenant_id, "pm_2")
                .await?,
            None
        );

        let entries = ports.payment_methods.detach_event_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].actor(), audit::Actor::System);
        assert_eq!(entries[0].action(), audit::Action::PaymentMethodDetached);
        Ok(())
    }

    #[tokio::test]
    async fn detached_for_an_unmirrored_card_is_unknown_payment_method()
    -> Result<(), Box<dyn Error>> {
        let ports = Ports::new();
        ports.seed_customer("cus_3");
        let ev = detached_event("cus_3", "pm_never", OffsetDateTime::now_utc());

        let outcome = apply_detached(
            &ports.customers,
            &ports.payment_methods,
            &ev,
            Uuid::new_v4(),
        )
        .await?;

        assert_eq!(
            outcome,
            EventOutcome::NotApplied(NotAppliedReason::UnknownPaymentMethod)
        );
        Ok(())
    }

    #[tokio::test]
    async fn stale_attached_does_not_regress_the_row() -> Result<(), Box<dyn Error>> {
        let ports = Ports::new();
        let (tenant_id, customer_id) = ports.seed_customer("cus_4");
        let newer = OffsetDateTime::now_utc();
        ports.payment_methods.seed(PaymentMethod {
            id: PaymentMethodId::new(Uuid::new_v4()),
            tenant_id,
            customer_id,
            stripe_payment_method_id: "pm_4".to_string(),
            brand: "visa".to_string(),
            last4: "4242".to_string(),
            is_default: false,
            last_event_created_at: Some(newer),
            created_at: newer,
            deleted_at: None,
        });

        let ev = attached_event("cus_4", "pm_4", newer - Duration::minutes(5));
        // The stale event names a different card brand; the row must not take it.
        let mut ev = ev;
        ev.payload["data"]["object"]["card"]["brand"] = json!("amex");

        let outcome = apply_attached(
            &ports.customers,
            &ports.payment_methods,
            &ev,
            Uuid::new_v4(),
        )
        .await?;

        assert_eq!(outcome, EventOutcome::NotApplied(NotAppliedReason::Stale));
        let found = ports
            .payment_methods
            .find_by_stripe_payment_method_id(tenant_id, "pm_4")
            .await?
            .ok_or("row exists")?;
        assert_eq!(found.brand, "visa");
        Ok(())
    }
}
