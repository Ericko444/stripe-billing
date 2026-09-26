use audit::{Action, Actor, CorrelationId, SubjectId, Target, TargetId};
use identity_domain::{
    AccountEvent, Clock, MembershipRepository, NewPassword, Password, PasswordHasher,
    PasswordPolicyError, SessionRepository, UserRepository, Verification,
};
use thiserror::Error;

use crate::{ActiveSession, AuthService};

/// Why a password change was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PasswordChangeError {
    /// The new password breaks the policy. Checked first, before anything
    /// else runs.
    #[error("new password rejected: {0}")]
    Policy(PasswordPolicyError),
    /// The current password did not verify.
    #[error("current password is wrong")]
    WrongCurrentPassword,
    /// Something this module depends on failed. The message is for logs.
    #[error("password change unavailable: {0}")]
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
    /// Changes the caller's password after they prove the current one.
    ///
    /// Every **other** session of the caller, in every tenant, ends; the
    /// session making the change survives -- its holder just proved the
    /// password, which a stolen cookie alone cannot do. Outstanding reset and
    /// invitation links stop working. All of it, with a `PasswordChanged` and
    /// a `SessionsRevoked` audit row per tenant, commits together.
    pub async fn change_password(
        &self,
        session: &ActiveSession,
        current: &Password,
        new: Password,
        correlation_id: CorrelationId,
    ) -> Result<(), PasswordChangeError> {
        let new = NewPassword::check(new).map_err(PasswordChangeError::Policy)?;
        let unavailable =
            |err: &dyn std::fmt::Display| PasswordChangeError::Unavailable(err.to_string());

        let user = self
            .users
            .find(session.user_id)
            .await
            .map_err(|err| unavailable(&err))?;
        let stored = user.as_ref().and_then(|user| user.password_hash.clone());
        let verification = self
            .hasher
            .verify(current, stored.as_ref().unwrap_or(self.hasher.dummy_hash()))
            .await
            .map_err(|err| unavailable(&err))?;
        if stored.is_none() || verification == Verification::Mismatch {
            return Err(PasswordChangeError::WrongCurrentPassword);
        }

        let hash = self
            .hasher
            .hash(&new)
            .await
            .map_err(|err| unavailable(&err))?;
        let now = self.clock.now();
        let subject = session.user_id.as_uuid();
        let event = |action| AccountEvent {
            actor: Actor::User(SubjectId::new(subject)),
            action,
            target: Target::User(TargetId::new(subject)),
            occurred_at: now,
            correlation_id,
        };
        self.users
            .change_password(
                session.user_id,
                &hash,
                session.id,
                &[
                    event(Action::PasswordChanged),
                    event(Action::SessionsRevoked),
                ],
            )
            .await
            .map_err(|err| unavailable(&err))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use identity_domain::{Email, PasswordHash, SessionId, User, UserId};
    use time::Duration;
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{FakeHasher, FakeMemberships, FakeSessions, FakeUsers, FixedClock};

    const CURRENT: &str = "correct horse battery staple";
    const NEW: &str = "a much longer and newer passphrase";

    struct World {
        service: AuthService<FakeUsers, FakeMemberships, FakeSessions, FakeHasher, FixedClock>,
        users: FakeUsers,
        hasher: FakeHasher,
        session: ActiveSession,
        clock: FixedClock,
    }

    fn world() -> Result<World, Box<dyn Error>> {
        let alice = User {
            id: UserId::new(Uuid::new_v4()),
            email: Email::parse("alice@example.test")?,
            display_name: String::new(),
            password_hash: Some(PasswordHash::new(FakeHasher::hash_of(CURRENT))),
            deactivated_at: None,
        };
        let users = FakeUsers::with(vec![alice.clone()]);
        let hasher = FakeHasher::default();
        let clock = FixedClock::at_epoch_plus_days(20_000);
        Ok(World {
            service: AuthService::new(
                users.clone(),
                FakeMemberships::default(),
                FakeSessions::default(),
                hasher.clone(),
                clock,
            ),
            users,
            hasher,
            session: ActiveSession {
                id: SessionId::new(Uuid::new_v4()),
                user_id: alice.id,
                tenant: None,
                authenticated_at: clock.now(),
                expires_at: clock.now() + Duration::hours(8),
            },
            clock,
        })
    }

    fn password(raw: &str) -> Password {
        Password::new(raw.to_string())
    }

    #[tokio::test]
    async fn a_change_keeps_this_session_and_audits_the_change_and_the_revocation()
    -> Result<(), Box<dyn Error>> {
        let w = world()?;
        let correlation_id = CorrelationId::new(Uuid::new_v4());

        w.service
            .change_password(
                &w.session,
                &password(CURRENT),
                password(NEW),
                correlation_id,
            )
            .await?;

        assert_eq!(
            w.users.password_changes(),
            vec![(w.session.user_id, FakeHasher::hash_of(NEW), w.session.id)]
        );
        let subject = w.session.user_id.as_uuid();
        let expected = |action| AccountEvent {
            actor: Actor::User(SubjectId::new(subject)),
            action,
            target: Target::User(TargetId::new(subject)),
            occurred_at: w.clock.now(),
            correlation_id,
        };
        assert_eq!(
            w.users.account_events(),
            vec![
                (w.session.user_id, expected(Action::PasswordChanged)),
                (w.session.user_id, expected(Action::SessionsRevoked)),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_wrong_current_password_changes_nothing() -> Result<(), Box<dyn Error>> {
        let w = world()?;

        let result = w
            .service
            .change_password(
                &w.session,
                &password("wrong horse battery staple"),
                password(NEW),
                CorrelationId::new(Uuid::new_v4()),
            )
            .await;

        assert_eq!(
            result.err(),
            Some(PasswordChangeError::WrongCurrentPassword)
        );
        assert!(w.users.password_changes().is_empty());
        assert!(w.users.account_events().is_empty());
        Ok(())
    }

    /// The policy is checked before the current password is even verified,
    /// so a caller cannot use a deliberately invalid new password to probe
    /// the current one for free.
    #[tokio::test]
    async fn the_policy_is_checked_before_the_current_password() -> Result<(), Box<dyn Error>> {
        let w = world()?;

        let result = w
            .service
            .change_password(
                &w.session,
                &password(CURRENT),
                password("short"),
                CorrelationId::new(Uuid::new_v4()),
            )
            .await;

        assert_eq!(
            result.err(),
            Some(PasswordChangeError::Policy(PasswordPolicyError::TooShort))
        );
        assert!(w.hasher.verified_against().is_empty());
        assert!(w.users.password_changes().is_empty());
        Ok(())
    }
}
