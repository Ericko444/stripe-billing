use domain::{
    CreateCustomerParams, CustomerSnapshot, DomainError, OutboundRequestRepository, TenantId,
};
use stripe::{IdempotencyKey, RequestStrategy, StripeRequest};
use stripe_core::customer::CreateCustomer;

use crate::ledger::Ledger;
use crate::{StripeError, fingerprint};

/// Encodes an optional input field so `None` and `Some("")` produce
/// genuinely different fingerprint inputs. Collapsing both to a bare empty
/// string would hide a real difference in the Stripe request body: `None`
/// omits the form field entirely, `Some("")` sends it present and empty --
/// two different requests that a naive encoding would fingerprint the same.
fn encode_optional(value: Option<&str>) -> String {
    match value {
        Some(v) => format!("S{v}"),
        None => "N".to_string(),
    }
}

/// `BillingProvider::create_customer`'s real implementation: fingerprint the
/// inputs, reserve an idempotency key, call Stripe with it, mark the
/// reservation complete, and return a snapshot.
pub async fn create_customer<R: OutboundRequestRepository>(
    client: &stripe::Client,
    ledger: &Ledger<R>,
    tenant_id: TenantId,
    params: CreateCustomerParams,
) -> Result<CustomerSnapshot, DomainError> {
    // Every field of CreateCustomerParams varies the request body, so every
    // field is in the fingerprint -- see CreateCustomerParams's own doc
    // comment in domain::billing_provider.
    let email_field = encode_optional(params.email.as_deref());
    let name_field = encode_optional(params.name.as_deref());
    let request_fingerprint = fingerprint("create_customer", &[&email_field, &name_field]);

    let reservation = ledger
        .reserve(tenant_id, "create_customer", &request_fingerprint)
        .await?;

    let key = IdempotencyKey::new(&reservation.idempotency_key)
        .map_err(|err| StripeError::Config(err.to_string()))
        .map_err(DomainError::from)?;

    let mut request = CreateCustomer::new();
    if let Some(email) = &params.email {
        request = request.email(email.clone());
    }
    if let Some(name) = &params.name {
        request = request.name(name.clone());
    }

    // The only acceptable request strategy for a mutating call: the key
    // came from the ledger, reserved *before* this call -- not minted fresh
    // here. RequestStrategy::idempotent_with_uuid(), IdempotencyKey::new_uuid_v4(),
    // RequestStrategy::Retry, and RequestStrategy::ExponentialBackoff all
    // mint a fresh uuid per attempt (request_strategy.rs:86), which is
    // precisely the non-idempotency this whole module exists to avoid.
    // This crate never uses any of the four.
    let customer = request
        .customize()
        .request_strategy(RequestStrategy::Idempotent(key))
        .send(client)
        .await
        .map_err(StripeError::from)
        .map_err(DomainError::from)?;

    // Not reached if the call above returned an error -- `?` exits first.
    ledger
        .complete(tenant_id, &reservation, customer.id.to_string())
        .await?;

    Ok(CustomerSnapshot {
        stripe_customer_id: customer.id.to_string(),
    })
}
