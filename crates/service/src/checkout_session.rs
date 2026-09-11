use domain::{BillingEvent, CustomerRepository, DomainError, TenantId, VerifiedEvent};
use serde_json::Value;
use uuid::Uuid;

use crate::webhook::{EventOutcome, NotAppliedReason};

/// Applies `checkout.session.completed` -- the bootstrap exception to
/// resolving the tenant from a local customer row. It links the Stripe
/// customer to a tenant by creating the `billing.customers` mirror row; the
/// subscription mirror arrives via the `customer.subscription.created`
/// Stripe sends alongside (which creates it now that the customer is known).
///
/// **The database lookup is always attempted first.** Only when
/// `find_by_stripe_customer_id` returns `None` -- a genuine first-time
/// bootstrap -- does [`tenant_from_session`] read a tenant from the payload,
/// and even then the two module-set sources must agree.
pub async fn apply<C>(customers: &C, event: &VerifiedEvent) -> Result<EventOutcome, DomainError>
where
    C: CustomerRepository,
{
    let fields = read_session(&event.payload)?;

    if customers
        .find_by_stripe_customer_id(&fields.stripe_customer_id)
        .await?
        .is_some()
    {
        // Already linked -- the session completing adds nothing to mirror.
        return Ok(EventOutcome::NotApplied(NotAppliedReason::Acknowledged));
    }

    let Some(tenant_id) = tenant_from_session(&fields)? else {
        // No local customer and no tenant hint on the session: nothing this
        // module can safely bootstrap from.
        return Ok(EventOutcome::NotApplied(NotAppliedReason::UnknownCustomer));
    };

    let customer = customers
        .create(tenant_id, Some(fields.stripe_customer_id.clone()))
        .await?;

    Ok(EventOutcome::Applied(BillingEvent::CheckoutCompleted {
        tenant_id,
        customer_id: customer.id,
    }))
}

/// Resolves the tenant from a Checkout session's module-set fields --
/// `client_reference_id` and `metadata.tenant_id`. **The only code path in
/// this module that trusts the payload for tenancy.**
///
/// Metadata can be edited from the Stripe dashboard, so it is not a general
/// trust anchor: this is reached *only* after `find_by_stripe_customer_id`
/// has returned `None`. As the available guard, the two sources must not
/// disagree: an event whose `metadata.tenant_id` contradicts its
/// `client_reference_id` is rejected, not trusted. Full session-ownership
/// validation -- proving this module created *this* session for *this*
/// tenant -- would need a ledger of created sessions; agreement is the
/// check made instead.
fn tenant_from_session(fields: &SessionFields) -> Result<Option<TenantId>, DomainError> {
    let from_ref = fields
        .client_reference_id
        .as_deref()
        .map(parse_tenant)
        .transpose()?;
    let from_meta = fields
        .metadata_tenant_id
        .as_deref()
        .map(parse_tenant)
        .transpose()?;

    match (from_ref, from_meta) {
        (Some(a), Some(b)) if a != b => Err(DomainError::MalformedEvent(
            "checkout session client_reference_id and metadata.tenant_id disagree".to_string(),
        )),
        (Some(t), _) | (None, Some(t)) => Ok(Some(t)),
        (None, None) => Ok(None),
    }
}

fn parse_tenant(raw: &str) -> Result<TenantId, DomainError> {
    Uuid::parse_str(raw).map(TenantId::new).map_err(|_| {
        DomainError::MalformedEvent(format!("checkout session tenant is not a uuid: {raw:?}"))
    })
}

/// The fields this handler needs off a `checkout.session.completed`
/// envelope's `data.object`.
struct SessionFields {
    stripe_customer_id: String,
    client_reference_id: Option<String>,
    metadata_tenant_id: Option<String>,
}

fn read_session(payload: &Value) -> Result<SessionFields, DomainError> {
    let object = payload
        .get("data")
        .and_then(|data| data.get("object"))
        .ok_or_else(|| {
            DomainError::MalformedEvent("event envelope missing data.object".to_string())
        })?;

    let stripe_customer_id = object
        .get("customer")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            DomainError::MalformedEvent(
                "checkout session object missing string `customer`".to_string(),
            )
        })?;

    let client_reference_id = object
        .get("client_reference_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let metadata_tenant_id = object
        .get("metadata")
        .and_then(|m| m.get("tenant_id"))
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(SessionFields {
        stripe_customer_id,
        client_reference_id,
        metadata_tenant_id,
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use domain::{Customer, CustomerId};
    use serde_json::json;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;
    use crate::test_support::InMemoryCustomers;

    fn session_event(
        stripe_customer_id: &str,
        client_reference_id: Option<&str>,
        metadata_tenant_id: Option<&str>,
    ) -> VerifiedEvent {
        let mut object = json!({ "id": "cs_test", "customer": stripe_customer_id });
        if let Some(cri) = client_reference_id {
            object["client_reference_id"] = json!(cri);
        }
        if let Some(mt) = metadata_tenant_id {
            object["metadata"] = json!({ "tenant_id": mt });
        }
        VerifiedEvent {
            id: domain::WebhookEventId::new(Uuid::new_v4()),
            stripe_event_id: format!("evt_{}", Uuid::new_v4()),
            event_type: "checkout.session.completed".to_string(),
            created: OffsetDateTime::now_utc(),
            payload: json!({
                "id": "evt_test",
                "type": "checkout.session.completed",
                "data": { "object": object },
            }),
        }
    }

    #[tokio::test]
    async fn existing_customer_link_is_acknowledged() -> Result<(), Box<dyn Error>> {
        let customers = InMemoryCustomers::default();
        let tenant_id = TenantId::new(Uuid::new_v4());
        customers.seed(Customer {
            id: CustomerId::new(Uuid::new_v4()),
            tenant_id,
            stripe_customer_id: Some("cus_known".to_string()),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        });

        let event = session_event("cus_known", Some(&Uuid::new_v4().to_string()), None);
        let outcome = apply(&customers, &event).await?;

        assert_eq!(
            outcome,
            EventOutcome::NotApplied(NotAppliedReason::Acknowledged)
        );
        Ok(())
    }

    #[tokio::test]
    async fn bootstrap_from_client_reference_id_creates_the_customer_link()
    -> Result<(), Box<dyn Error>> {
        let customers = InMemoryCustomers::default();
        let tenant_id = TenantId::new(Uuid::new_v4());

        let event = session_event("cus_new", Some(&tenant_id.as_uuid().to_string()), None);
        let outcome = apply(&customers, &event).await?;

        assert_eq!(
            outcome,
            EventOutcome::Applied(BillingEvent::CheckoutCompleted {
                tenant_id,
                customer_id: customers
                    .find_by_stripe_customer_id("cus_new")
                    .await?
                    .ok_or("linked")?
                    .id,
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn database_lookup_wins_over_a_conflicting_client_reference_id()
    -> Result<(), Box<dyn Error>> {
        let customers = InMemoryCustomers::default();
        let real_tenant = TenantId::new(Uuid::new_v4());
        customers.seed(Customer {
            id: CustomerId::new(Uuid::new_v4()),
            tenant_id: real_tenant,
            stripe_customer_id: Some("cus_owned".to_string()),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        });

        // A payload claiming a different tenant -- the DB path runs first and
        // never consults it.
        let attacker = Uuid::new_v4().to_string();
        let event = session_event("cus_owned", Some(&attacker), None);
        let outcome = apply(&customers, &event).await?;

        assert_eq!(
            outcome,
            EventOutcome::NotApplied(NotAppliedReason::Acknowledged)
        );
        assert_eq!(customers.list(real_tenant).await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn disagreeing_tenant_sources_are_rejected() -> Result<(), Box<dyn Error>> {
        let customers = InMemoryCustomers::default();
        let event = session_event(
            "cus_conflict",
            Some(&Uuid::new_v4().to_string()),
            Some(&Uuid::new_v4().to_string()),
        );

        let outcome = apply(&customers, &event).await;

        assert!(matches!(outcome, Err(DomainError::MalformedEvent(_))));
        assert!(
            customers
                .list(TenantId::new(Uuid::new_v4()))
                .await?
                .is_empty()
        );
        Ok(())
    }

    #[tokio::test]
    async fn no_customer_and_no_tenant_hint_is_unknown_customer() -> Result<(), Box<dyn Error>> {
        let customers = InMemoryCustomers::default();
        let event = session_event("cus_orphan", None, None);

        let outcome = apply(&customers, &event).await?;

        assert_eq!(
            outcome,
            EventOutcome::NotApplied(NotAppliedReason::UnknownCustomer)
        );
        Ok(())
    }
}
