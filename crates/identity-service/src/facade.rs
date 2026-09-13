use async_trait::async_trait;
use audit::CorrelationId;
use identity_domain::{
    Clock, Email, Mailer, MembershipRepository, Password, PasswordHasher, PasswordTokenRepository,
    SessionRepository, TenantId, UserRepository,
};

use crate::{
    ActiveSession, AuthService, CompleteResetError, LoginError, LoginOutcome, Me,
    PasswordChangeError, PasswordResetService, ProfileError, SelectTenantError, SessionError,
    TenantSelection,
};

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

    /// See [`AuthService::select_tenant`].
    async fn select_tenant(
        &self,
        session: &ActiveSession,
        tenant_id: TenantId,
        correlation_id: CorrelationId,
    ) -> Result<TenantSelection, SelectTenantError>;

    /// See [`AuthService::logout`].
    async fn logout(&self, session: &ActiveSession) -> Result<(), SessionError>;

    /// See [`AuthService::change_password`].
    async fn change_password(
        &self,
        session: &ActiveSession,
        current: &Password,
        new: Password,
        correlation_id: CorrelationId,
    ) -> Result<(), PasswordChangeError>;

    /// See [`AuthService::update_display_name`].
    async fn update_display_name(
        &self,
        session: &ActiveSession,
        display_name: &str,
        correlation_id: CorrelationId,
    ) -> Result<Me, ProfileError>;
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

    async fn select_tenant(
        &self,
        session: &ActiveSession,
        tenant_id: TenantId,
        correlation_id: CorrelationId,
    ) -> Result<TenantSelection, SelectTenantError> {
        AuthService::select_tenant(self, session, tenant_id, correlation_id).await
    }

    async fn logout(&self, session: &ActiveSession) -> Result<(), SessionError> {
        AuthService::logout(self, session).await
    }

    async fn update_display_name(
        &self,
        session: &ActiveSession,
        display_name: &str,
        correlation_id: CorrelationId,
    ) -> Result<Me, ProfileError> {
        AuthService::update_display_name(self, session, display_name, correlation_id).await
    }

    async fn change_password(
        &self,
        session: &ActiveSession,
        current: &Password,
        new: Password,
        correlation_id: CorrelationId,
    ) -> Result<(), PasswordChangeError> {
        AuthService::change_password(self, session, current, new, correlation_id).await
    }
}

/// Completing a password reset, as an object-safe trait for the router --
/// the same façade role as [`Authentication`], for the other service.
#[async_trait]
pub trait PasswordResets: Send + Sync {
    /// See [`PasswordResetService::complete`].
    async fn complete_reset(
        &self,
        presented: &str,
        new_password: Password,
        correlation_id: CorrelationId,
    ) -> Result<(), CompleteResetError>;
}

#[async_trait]
impl<U, T, H, M, C> PasswordResets for PasswordResetService<U, T, H, M, C>
where
    U: UserRepository,
    T: PasswordTokenRepository,
    H: PasswordHasher,
    M: Mailer,
    C: Clock,
{
    async fn complete_reset(
        &self,
        presented: &str,
        new_password: Password,
        correlation_id: CorrelationId,
    ) -> Result<(), CompleteResetError> {
        PasswordResetService::complete(self, presented, new_password, correlation_id).await
    }
}
