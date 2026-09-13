use async_trait::async_trait;
use audit::CorrelationId;
use identity_domain::{
    Clock, Email, MembershipRepository, Password, PasswordHasher, SessionRepository, UserRepository,
};

use crate::{ActiveSession, AuthService, LoginError, LoginOutcome, Me, SessionError};

/// The identity use cases as an object-safe trait, so a router can hold
/// `Arc<dyn Authentication>` without naming five type parameters -- the
/// same façade role `service::Reads` and `service::Writes` play for billing.
#[async_trait]
pub trait Authentication: Send + Sync {
    /// See [`AuthService::login`].
    async fn login(
        &self,
        email: &Email,
        password: &Password,
        correlation_id: CorrelationId,
    ) -> Result<LoginOutcome, LoginError>;

    /// See [`AuthService::authenticate`].
    async fn authenticate(&self, presented: &str) -> Result<ActiveSession, SessionError>;

    /// See [`AuthService::me`].
    async fn me(&self, session: &ActiveSession) -> Result<Me, SessionError>;
}

#[async_trait]
impl<U, M, S, H, C> Authentication for AuthService<U, M, S, H, C>
where
    U: UserRepository,
    M: MembershipRepository,
    S: SessionRepository,
    H: PasswordHasher,
    C: Clock,
{
    async fn login(
        &self,
        email: &Email,
        password: &Password,
        correlation_id: CorrelationId,
    ) -> Result<LoginOutcome, LoginError> {
        AuthService::login(self, email, password, correlation_id).await
    }

    async fn authenticate(&self, presented: &str) -> Result<ActiveSession, SessionError> {
        AuthService::authenticate(self, presented).await
    }

    async fn me(&self, session: &ActiveSession) -> Result<Me, SessionError> {
        AuthService::me(self, session).await
    }
}
