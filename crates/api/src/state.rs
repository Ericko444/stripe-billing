use std::sync::Arc;

use domain::WebhookVerifier;
use service::{Reads, WebhookHandler, Writes};

/// The success/cancel URLs a Checkout Session redirects the customer to.
///
/// **Host config, never a request field.** Stripe requires both. Taking
/// them from the request body would let a caller redirect a customer
/// anywhere after payment -- an open redirect in the one flow where the
/// user is most primed to trust the destination -- so `api` reads them only
/// from here, set once when the host builds [`AppState`].
#[derive(Clone, Debug)]
pub struct CheckoutUrls {
    /// Where Stripe returns the customer after a completed checkout.
    pub success: String,
    /// Where Stripe returns the customer if they abandon checkout.
    pub cancel: String,
}

/// State shared across every route in the billing router, reached through
/// Axum's `State` extractor.
///
/// The three service fields are `Arc<dyn Trait>` -- chosen by the host at
/// runtime, not known to this crate at compile time -- so `AppState` stays a
/// fixed, concrete type regardless of which concrete verifier, handler, read
/// or write service a host wires in. `Clone` is a pointer copy (plus two
/// `String` clones for [`CheckoutUrls`]), which is what Axum needs to hand a
/// copy of the state to every request.
///
/// A host that mounts only [`webhook_router`](crate::webhook_router) still
/// constructs a full `AppState`; `reads`, `writes` and `checkout_urls`
/// simply go untouched on that path.
///
/// Holds no secret: the signing secret `WebhookVerifier` needs lives inside
/// whatever adapter implements it, never here.
#[derive(Clone)]
pub struct AppState {
    /// Verifies and dedups inbound Stripe webhooks.
    pub webhook_verifier: Arc<dyn WebhookVerifier>,
    /// Processes a verified, deduplicated webhook event.
    pub webhook_handler: Arc<dyn WebhookHandler>,
    /// The tenant-scoped read use cases behind the `GET` routes.
    pub reads: Arc<dyn Reads>,
    /// The tenant-scoped write use cases behind the `POST`/`DELETE` routes.
    pub writes: Arc<dyn Writes>,
    /// The Checkout Session redirect URLs, from host config.
    pub checkout_urls: CheckoutUrls,
}

impl AppState {
    /// Builds application state from the webhook path's two dependencies, the
    /// read and write services the tenant-scoped routes need, and the
    /// Checkout redirect URLs.
    pub fn new(
        webhook_verifier: Arc<dyn WebhookVerifier>,
        webhook_handler: Arc<dyn WebhookHandler>,
        reads: Arc<dyn Reads>,
        writes: Arc<dyn Writes>,
        checkout_urls: CheckoutUrls,
    ) -> Self {
        Self {
            webhook_verifier,
            webhook_handler,
            reads,
            writes,
            checkout_urls,
        }
    }
}
