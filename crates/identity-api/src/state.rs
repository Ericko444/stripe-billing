use std::sync::Arc;

use identity_service::Authentication;

/// What the identity routes and [`AuthenticatedSession`] need, carried as a
/// request extension.
///
/// An extension rather than router state so the extractor works on routers
/// whose state is someone else's type: a host installs it once, over every
/// router it merges, with `.layer(Extension(state))`. [`identity_router`]
/// installs it on its own routes itself.
///
/// [`AuthenticatedSession`]: crate::AuthenticatedSession
/// [`identity_router`]: crate::identity_router
#[derive(Clone)]
pub struct IdentityState {
    authentication: Arc<dyn Authentication>,
}

impl IdentityState {
    /// State over the given use cases.
    pub fn new(authentication: Arc<dyn Authentication>) -> Self {
        Self { authentication }
    }

    /// The use cases.
    pub fn authentication(&self) -> &dyn Authentication {
        self.authentication.as_ref()
    }
}
