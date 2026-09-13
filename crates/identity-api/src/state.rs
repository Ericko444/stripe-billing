use std::net::IpAddr;
use std::sync::Arc;

use identity_domain::RateLimiter;
use identity_service::{Authentication, Members, PasswordResets, ResetQueue};

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
    password_resets: Arc<dyn PasswordResets>,
    members: Arc<dyn Members>,
    limiter: Arc<dyn RateLimiter>,
    reset_queue: ResetQueue,
    trusted_proxy: Option<IpAddr>,
}

impl IdentityState {
    /// State over the session, password reset and members use cases, the
    /// rate limiter and the queue feeding the reset worker, trusting no
    /// proxy.
    pub fn new(
        authentication: Arc<dyn Authentication>,
        password_resets: Arc<dyn PasswordResets>,
        members: Arc<dyn Members>,
        limiter: Arc<dyn RateLimiter>,
        reset_queue: ResetQueue,
    ) -> Self {
        Self {
            authentication,
            password_resets,
            members,
            limiter,
            reset_queue,
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

    /// Completing password resets.
    pub fn password_resets(&self) -> &dyn PasswordResets {
        self.password_resets.as_ref()
    }

    /// Managing the session tenant's members.
    pub fn members(&self) -> &dyn Members {
        self.members.as_ref()
    }

    /// The rate limiter.
    pub fn limiter(&self) -> &dyn RateLimiter {
        self.limiter.as_ref()
    }

    /// The proxy whose `X-Forwarded-For` is believed, if any.
    pub fn trusted_proxy(&self) -> Option<IpAddr> {
        self.trusted_proxy
    }

    /// What the reset request handler may touch, and all it may touch: the
    /// rate limiter and the queue. No use case, and so no account data --
    /// which is how "the response cannot depend on whether the account
    /// exists" is a property of the code rather than of its timing.
    pub fn reset_requests(&self) -> ResetRequests<'_> {
        ResetRequests {
            limiter: self.limiter.as_ref(),
            queue: &self.reset_queue,
        }
    }
}

/// The narrow view of [`IdentityState`] the reset request handler works
/// through. See [`IdentityState::reset_requests`].
pub struct ResetRequests<'a> {
    /// Counts requests per address and per IP.
    pub limiter: &'a dyn RateLimiter,
    /// Hands the address to the worker.
    pub queue: &'a ResetQueue,
}
