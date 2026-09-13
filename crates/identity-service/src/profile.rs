use audit::{Action, Actor, CorrelationId, SubjectId, Target, TargetId};
use identity_domain::{
    AccountEvent, Clock, DisplayName, MembershipRepository, PasswordHasher, SessionRepository,
    UserRepository,
};
use thiserror::Error;

use crate::{ActiveSession, AuthService, Me, SessionError};

/// Why a profile change was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProfileError {
    /// The display name was too long or contained control characters.
    #[error("invalid display name")]
    InvalidDisplayName,
    /// The account behind the session no longer exists.
    #[error("not authenticated")]
    Unauthenticated,
    /// Something this module depends on failed. The message is for logs.
    #[error("profile update unavailable: {0}")]
    Unavailable(String),
}

impl<U, M, S, H, C> AuthService<U, M, S, H, C>
where
    U: UserRepository,
    M: MembershipRepository,
    S: SessionRepository,
    H: PasswordHasher,
    C: Clock,
{
    /// Sets the caller's display name and returns the updated account.
    ///
    /// An account-level change: audited as `UserUpdated` once per tenant the
    /// caller is an active member of, in the update's own transaction.
    pub async fn update_display_name(
        &self,
        session: &ActiveSession,
        raw: &str,
        correlation_id: CorrelationId,
    ) -> Result<Me, ProfileError> {
        let display_name = DisplayName::parse(raw).map_err(|_| ProfileError::InvalidDisplayName)?;
        let user = session.user_id.as_uuid();
        let event = AccountEvent {
            actor: Actor::User(SubjectId::new(user)),
            action: Action::UserUpdated,
            target: Target::User(TargetId::new(user)),
            occurred_at: self.clock.now(),
            correlation_id,
        };
        self.users
            .update_display_name(session.user_id, &display_name, &event)
            .await
            .map_err(|err| ProfileError::Unavailable(err.to_string()))?;

        self.me(session).await.map_err(|err| match err {
            SessionError::Unauthenticated => ProfileError::Unauthenticated,
            SessionError::Unavailable(reason) => ProfileError::Unavailable(reason),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use identity_domain::{Email, SessionId, User, UserId};
    use time::Duration;
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{FakeHasher, FakeMemberships, FakeSessions, FakeUsers, FixedClock};

    fn alice() -> Result<User, Box<dyn Error>> {
        Ok(User {
            id: UserId::new(Uuid::new_v4()),
            email: Email::parse("alice@example.test")?,
            display_name: "Alice".to_string(),
            password_hash: None,
            deactivated_at: None,
        })
    }

    #[tokio::test]
    async fn an_update_is_audited_as_the_user_acting_on_themselves() -> Result<(), Box<dyn Error>> {
        let alice = alice()?;
        let users = FakeUsers::with(vec![alice.clone()]);
        let clock = FixedClock::at_epoch_plus_days(20_000);
        let service = AuthService::new(
            users.clone(),
            FakeMemberships::default(),
            FakeSessions::default(),
            FakeHasher::default(),
            clock,
        );
        let session = ActiveSession {
            id: SessionId::new(Uuid::new_v4()),
            user_id: alice.id,
            tenant: None,
            authenticated_at: clock.now(),
            expires_at: clock.now() + Duration::hours(8),
        };
        let correlation_id = CorrelationId::new(Uuid::new_v4());

        let me = service
            .update_display_name(&session, "  Alice L.  ", correlation_id)
            .await?;

        assert_eq!(me.display_name, "Alice L.");
        let user = alice.id.as_uuid();
        assert_eq!(
            users.account_events(),
            vec![(
                alice.id,
                AccountEvent {
                    actor: Actor::User(SubjectId::new(user)),
                    action: Action::UserUpdated,
                    target: Target::User(TargetId::new(user)),
                    occurred_at: clock.now(),
                    correlation_id,
                }
            )]
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_invalid_name_changes_nothing() -> Result<(), Box<dyn Error>> {
        let alice = alice()?;
        let users = FakeUsers::with(vec![alice.clone()]);
        let clock = FixedClock::at_epoch_plus_days(20_000);
        let service = AuthService::new(
            users.clone(),
            FakeMemberships::default(),
            FakeSessions::default(),
            FakeHasher::default(),
            clock,
        );
        let session = ActiveSession {
            id: SessionId::new(Uuid::new_v4()),
            user_id: alice.id,
            tenant: None,
            authenticated_at: clock.now(),
            expires_at: clock.now() + Duration::hours(8),
        };

        let result = service
            .update_display_name(&session, "line\nbreak", CorrelationId::new(Uuid::new_v4()))
            .await;

        assert_eq!(result.err(), Some(ProfileError::InvalidDisplayName));
        assert!(users.account_events().is_empty());
        Ok(())
    }
}
