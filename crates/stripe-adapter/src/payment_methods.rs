use domain::{DomainError, OutboundRequestRepository, TenantId};
use stripe::{RequestStrategy, StripeRequest};
use stripe_core::customer::{UpdateCustomer, UpdateCustomerInvoiceSettings};
use stripe_payment::payment_method::DetachPaymentMethod;

use crate::ledger::{Ledger, idempotency_key};
use crate::{StripeError, fingerprint};

/// `BillingProvider::set_default_payment_method`'s real implementation.
///
/// A customer update setting `invoice_settings.default_payment_method`. Same
/// shape as every other mutating call: fingerprint the inputs, reserve an
/// idempotency key *before* the call, send it with `RequestStrategy::Idempotent`,
/// mark the reservation complete.
///
/// The fingerprint covers the customer id and the payment method id -- the
/// two values that vary the request. Returns `()`: the caller already knows
/// which method it asked for, and `service` reconciles the local mirror
/// itself rather than reading it back from the customer object here.
pub async fn set_default_payment_method<R: OutboundRequestRepository>(
    client: &stripe::Client,
    ledger: &Ledger<R>,
    tenant_id: TenantId,
    stripe_customer_id: &str,
    stripe_payment_method_id: &str,
) -> Result<(), DomainError> {
    let request_fingerprint = fingerprint(
        "set_default_payment_method",
        &[stripe_customer_id, stripe_payment_method_id],
    );

    let reservation = ledger
        .reserve(
            tenant_id,
            "set_default_payment_method",
            &request_fingerprint,
        )
        .await?;

    UpdateCustomer::new(stripe_customer_id)
        .invoice_settings(UpdateCustomerInvoiceSettings {
            default_payment_method: Some(stripe_payment_method_id.to_string()),
            ..UpdateCustomerInvoiceSettings::new()
        })
        .customize()
        .request_strategy(RequestStrategy::Idempotent(idempotency_key(&reservation)?))
        .send(client)
        .await
        .map_err(StripeError::from)
        .map_err(DomainError::from)?;

    ledger
        .complete(tenant_id, &reservation, stripe_customer_id.to_string())
        .await?;

    Ok(())
}

/// `BillingProvider::detach_payment_method`'s real implementation.
///
/// `POST /v1/payment_methods/{id}/detach`. Permanent at Stripe; `service`
/// soft-deletes the mirror row *after* this returns `Ok`.
///
/// **The fingerprint is tenant + payment method id only -- no timestamp.**
/// A retry inside the ~23h key window replays the original detach instead of
/// issuing a new one, so it cannot detach a card the customer re-added in
/// the meantime. This is the one operation where minting a fresh key on an
/// unknown-outcome retry would be actively dangerous, which is why the
/// ledger's case E (an incomplete row past the window) returns a typed
/// "reconcile first" error rather than guessing.
pub async fn detach_payment_method<R: OutboundRequestRepository>(
    client: &stripe::Client,
    ledger: &Ledger<R>,
    tenant_id: TenantId,
    stripe_payment_method_id: &str,
) -> Result<(), DomainError> {
    let request_fingerprint = fingerprint("detach_payment_method", &[stripe_payment_method_id]);

    let reservation = ledger
        .reserve(tenant_id, "detach_payment_method", &request_fingerprint)
        .await?;

    DetachPaymentMethod::new(stripe_payment_method_id)
        .customize()
        .request_strategy(RequestStrategy::Idempotent(idempotency_key(&reservation)?))
        .send(client)
        .await
        .map_err(StripeError::from)
        .map_err(DomainError::from)?;

    ledger
        .complete(
            tenant_id,
            &reservation,
            stripe_payment_method_id.to_string(),
        )
        .await?;

    Ok(())
}
