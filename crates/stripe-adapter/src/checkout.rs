use domain::{
    CheckoutSessionParams, CheckoutSessionSnapshot, DomainError, OutboundRequestRepository,
    TenantId,
};
use stripe::{RequestStrategy, StripeRequest};
use stripe_checkout::CheckoutSessionMode;
use stripe_checkout::checkout_session::{CreateCheckoutSession, CreateCheckoutSessionLineItems};

use crate::ledger::{Ledger, idempotency_key};
use crate::{StripeError, fingerprint};

/// `BillingProvider::create_checkout_session`'s real implementation. Same
/// shape as every other mutating call: fingerprint the inputs, reserve an
/// idempotency key *before* the call, send it with `RequestStrategy::Idempotent`,
/// mark the reservation complete.
///
/// The fingerprint covers **the customer id and the price id** -- the two
/// inputs that identify the operation. The two URLs vary the request body
/// but come from host config, constant per deployment, so they are not part
/// of the call's identity (`docs/spec/phase-4c-write-routes.md` decision 2).
///
/// Mode is `subscription`. There is **no mirror write** here -- the local
/// `subscriptions` row is created later, by the
/// `customer.subscription.created` webhook, once the customer completes
/// checkout. The returned `url` is the hosted page and goes straight back
/// through the caller to the browser (per `CheckoutSessionSnapshot`'s docs,
/// into no log line).
pub async fn create_checkout_session<R: OutboundRequestRepository>(
    client: &stripe::Client,
    ledger: &Ledger<R>,
    tenant_id: TenantId,
    params: CheckoutSessionParams,
) -> Result<CheckoutSessionSnapshot, DomainError> {
    let request_fingerprint = fingerprint(
        "create_checkout_session",
        &[&params.stripe_customer_id, &params.stripe_price_id],
    );

    let reservation = ledger
        .reserve(tenant_id, "create_checkout_session", &request_fingerprint)
        .await?;

    let line_items = vec![CreateCheckoutSessionLineItems {
        price: Some(params.stripe_price_id.clone()),
        quantity: Some(1),
        ..CreateCheckoutSessionLineItems::new()
    }];
    let session = CreateCheckoutSession::new()
        .mode(CheckoutSessionMode::Subscription)
        .customer(params.stripe_customer_id.as_str())
        .line_items(line_items)
        .success_url(params.success_url.as_str())
        .cancel_url(params.cancel_url.as_str())
        .customize()
        .request_strategy(RequestStrategy::Idempotent(idempotency_key(&reservation)?))
        .send(client)
        .await
        .map_err(StripeError::from)
        .map_err(DomainError::from)?;

    // Stripe types `url` as optional (it is `None` for non-hosted sessions);
    // a `subscription`-mode session always has one. A response without it is
    // one we cannot fulfil the caller's request from.
    let url = session.url.ok_or_else(|| {
        DomainError::from(StripeError::Deserialization(
            "checkout session response carried no url".to_string(),
        ))
    })?;

    ledger
        .complete(tenant_id, &reservation, session.id.to_string())
        .await?;

    Ok(CheckoutSessionSnapshot {
        url,
        stripe_session_id: session.id.to_string(),
    })
}
