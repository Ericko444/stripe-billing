use std::net::IpAddr;
use std::sync::Arc;

use identity_domain::RateLimiter;
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
    limiter: Arc<dyn RateLimiter>,
    trusted_proxy: Option<IpAddr>,
}

impl IdentityState {
    /// State over the given use cases and rate limiter, trusting no proxy.
    pub fn new(authentication: Arc<dyn Authentication>, limiter: Arc<dyn RateLimiter>) -> Self {
        Self {
            authentication,
            limiter,
            trusted_proxy: None,
        }
    }

    /// Trusts `X-Forwarded-For` when -- and only when -- the socket peer is
    /// `proxy`.
    pub fn with_trusted_proxy(mut self, proxy: IpAddr) -> Self {
        self.trusted_proxy = Some(proxy);
        self
    }

    /// The use cases.
    pub fn authentication(&self) -> &dyn Authentication {
        self.authentication.as_ref()
    }

    /// The rate limiter.
    pub fn limiter(&self) -> &dyn RateLimiter {
        self.limiter.as_ref()
    }

    /// The proxy whose `X-Forwarded-For` is believed, if any.
    pub fn trusted_proxy(&self) -> Option<IpAddr> {
        self.trusted_proxy
    }
}
