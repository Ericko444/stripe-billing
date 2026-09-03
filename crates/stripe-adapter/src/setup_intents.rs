use domain::{DomainError, OutboundRequestRepository, SetupIntentSnapshot, TenantId};
use stripe::{RequestStrategy, StripeRequest};
use stripe_core::setup_intent::CreateSetupIntent;

use crate::ledger::{Ledger, idempotency_key};
use crate::{StripeError, fingerprint};

/// The payment method types this SetupIntent is scoped to. `card` only,
/// deliberately, not left to automatic payment methods: the frontend's
/// `PaymentElement` confirms inline with no `return_url` (Phase 5 D4), which
/// only holds for a payment method that never redirects. Left unset,
/// Stripe's default surfaced wallets like Naver Pay first in this account's
/// test mode -- discovered live, not predicted -- which `confirmSetup` has no
/// redirect landing page to come back to.
///
/// Named once and used **twice**: in the request body and in the
/// fingerprint. Keeping the two together is the point. A value that shapes
/// the request but not the fingerprint is exactly the drift
/// [`fingerprint`]'s own docs warn about -- change it while a ledger key
/// reserved for the old body is still inside the ledger's key window, and
/// the next call replays that key at Stripe with different parameters.
/// Stripe rejects that with `400 Keys for idempotent requests can only be
/// used with the same parameters they were first used with`, and every
/// `POST /payment-methods/setup-intent` fails until the window passes.
///
/// That is not hypothetical: it happened on 2026-09-03, against keys
/// reserved roughly 35 minutes before commit `15fb944` introduced this
/// field.
const PAYMENT_METHOD_TYPES: [&str; 1] = ["card"];

/// `BillingProvider::create_setup_intent`'s real implementation. Same shape
/// as the customer and subscription methods: fingerprint the inputs,
/// reserve an idempotency key, call Stripe with it, mark the reservation
/// complete, return a snapshot.
///
/// The fingerprint covers both inputs that vary the request body: the
/// customer id and [`PAYMENT_METHOD_TYPES`]. There is **no mirror write** --
/// a SetupIntent has no local row; its `client_secret` goes straight back
/// through the caller to the browser (and, per `SetupIntentSnapshot`'s docs,
/// into no log line).
pub async fn create_setup_intent<R: OutboundRequestRepository>(
    client: &stripe::Client,
    ledger: &Ledger<R>,
    tenant_id: TenantId,
    stripe_customer_id: &str,
) -> Result<SetupIntentSnapshot, DomainError> {
    let mut fields = Vec::with_capacity(1 + PAYMENT_METHOD_TYPES.len());
    fields.push(stripe_customer_id);
    fields.extend_from_slice(&PAYMENT_METHOD_TYPES);
    let request_fingerprint = fingerprint("create_setup_intent", &fields);

    let reservation = ledger
        .reserve(tenant_id, "create_setup_intent", &request_fingerprint)
        .await?;

    let setup_intent = CreateSetupIntent::new()
        .customer(stripe_customer_id)
        .payment_method_types(PAYMENT_METHOD_TYPES.map(|kind| kind.to_string()))
        .customize()
        .request_strategy(RequestStrategy::Idempotent(idempotency_key(&reservation)?))
        .send(client)
        .await
        .map_err(StripeError::from)
        .map_err(DomainError::from)?;

    // Stripe types `client_secret` as optional, but a freshly created
    // SetupIntent always carries one. A response without it is one we
    // cannot fulfil the caller's request from -- an error, never an empty
    // string handed to the frontend.
    let client_secret = setup_intent.client_secret.ok_or_else(|| {
        DomainError::from(StripeError::Deserialization(
            "setup_intent response carried no client_secret".to_string(),
        ))
    })?;

    ledger
        .complete(tenant_id, &reservation, setup_intent.id.to_string())
        .await?;

    Ok(SetupIntentSnapshot { client_secret })
}
