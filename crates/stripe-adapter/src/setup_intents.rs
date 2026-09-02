use domain::{DomainError, OutboundRequestRepository, SetupIntentSnapshot, TenantId};
use stripe::{RequestStrategy, StripeRequest};
use stripe_core::setup_intent::CreateSetupIntent;

use crate::ledger::{Ledger, idempotency_key};
use crate::{StripeError, fingerprint};

/// `BillingProvider::create_setup_intent`'s real implementation. Same shape
/// as the customer and subscription methods: fingerprint the inputs,
/// reserve an idempotency key, call Stripe with it, mark the reservation
/// complete, return a snapshot.
///
/// The only input that varies the request body is the customer id, so that
/// is the whole fingerprint. There is **no mirror write** -- a SetupIntent
/// has no local row; its `client_secret` goes straight back through the
/// caller to the browser (and, per `SetupIntentSnapshot`'s docs, into no
/// log line).
pub async fn create_setup_intent<R: OutboundRequestRepository>(
    client: &stripe::Client,
    ledger: &Ledger<R>,
    tenant_id: TenantId,
    stripe_customer_id: &str,
) -> Result<SetupIntentSnapshot, DomainError> {
    let request_fingerprint = fingerprint("create_setup_intent", &[stripe_customer_id]);

    let reservation = ledger
        .reserve(tenant_id, "create_setup_intent", &request_fingerprint)
        .await?;

    let setup_intent = CreateSetupIntent::new()
        .customer(stripe_customer_id)
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
