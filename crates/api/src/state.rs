use std::sync::Arc;

use domain::WebhookVerifier;
use service::{Reads, WebhookHandler};

/// State shared across every route in the billing router, reached through
/// Axum's `State` extractor.
///
/// Every field is `Arc<dyn Trait>` -- chosen by the host at runtime, not
/// known to this crate at compile time -- so `AppState` stays a fixed,
/// concrete type regardless of which concrete verifier, handler or read
/// service a host wires in. `Clone` is a pointer copy, which is what Axum
/// needs to hand a copy of the state to every request.
///
/// A host that mounts only [`webhook_router`](crate::webhook_router) still
/// constructs a full `AppState`; `reads` simply goes untouched on that path.
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
}

impl AppState {
    /// Builds application state from the webhook path's two dependencies and
    /// the read service the tenant-scoped routes need.
    pub fn new(
        webhook_verifier: Arc<dyn WebhookVerifier>,
        webhook_handler: Arc<dyn WebhookHandler>,
        reads: Arc<dyn Reads>,
    ) -> Self {
        Self {
            webhook_verifier,
            webhook_handler,
            reads,
        }
    }
}
