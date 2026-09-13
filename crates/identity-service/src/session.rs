use identity_domain::{
    Clock, Email, Membership, MembershipRepository, PasswordHasher, SessionId, SessionRepository,
    SessionTenant, SplitToken, UserId, UserRepository,
};
use thiserror::Error;
use time::OffsetDateTime;

use crate::AuthService;

/// A session that was presented, found, verified and unexpired -- what an
/// authenticated request knows about its caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSession {
    /// The session row.
    pub id: SessionId,
    /// Who the caller is.
    pub user_id: UserId,
    /// The tenant the session is scoped to and the caller's role in it, read
    /// from the membership on this request; `None` before a tenant is picked.
    pub tenant: Option<SessionTenant>,
    /// When the caller proved a password.
    pub authenticated_at: OffsetDateTime,
    /// When the session stops being accepted.
    pub expires_at: OffsetDateTime,
}

/// The caller's own account, as `GET /auth/me` returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Me {
    /// Who the caller is.
    pub user_id: UserId,
    /// The caller's address.
    pub email: Email,
    /// The caller's display name.
    pub display_name: String,
    /// The tenant the session is scoped to, if any.
    pub tenant: Option<SessionTenant>,
    /// Every active membership.
    pub memberships: Vec<Membership>,
    /// When the session stops being accepted.
    pub expires_at: OffsetDateTime,
}

/// Why a presented session was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionError {
    /// Malformed token, unknown selector, wrong verifier, expired, user
    /// deactivated, membership suspended -- **one variant for all**.
    #[error("not authenticated")]
    Unauthenticated,
    /// Something this module depends on failed. The message is for logs.
    #[error("session lookup unavailable: {0}")]
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
    /// Accepts or rejects the token a request presented.
    ///
    /// Four checks, and every failure is the same error: the token parses;
    /// its selector finds a session whose user and (if scoped) membership are
    /// active; its verifier matches the stored hash, compared in constant
    /// time; and the session has not expired by the module's one clock.
    pub async fn authenticate(&self, presented: &str) -> Result<ActiveSession, SessionError> {
        let token = SplitToken::parse(presented).map_err(|_| SessionError::Unauthenticated)?;
        let stored = self
            .sessions
            .resolve(token.selector())
            .await
            .map_err(|err| SessionError::Unavailable(err.to_string()))?
            .ok_or(SessionError::Unauthenticated)?;

        if !stored.verifier_hash.verifies(token.verifier()) {
            return Err(SessionError::Unauthenticated);
        }
        if stored.expires_at <= self.clock.now() {
            return Err(SessionError::Unauthenticated);
        }

        Ok(ActiveSession {
            id: stored.id,
            user_id: stored.user_id,
            tenant: stored.tenant,
            authenticated_at: stored.authenticated_at,
            expires_at: stored.expires_at,
        })
    }

    /// The caller's own account and memberships.
    pub async fn me(&self, session: &ActiveSession) -> Result<Me, SessionError> {
        let unavailable =
            |err: identity_domain::RepositoryError| SessionError::Unavailable(err.to_string());
        let user = self
            .users
            .find(session.user_id)
            .await
            .map_err(unavailable)?
            .ok_or(SessionError::Unauthenticated)?;
        let memberships = self
            .memberships
            .active_for_user(session.user_id)
            .await
            .map_err(unavailable)?;

        Ok(Me {
            user_id: user.id,
            email: user.email,
            display_name: user.display_name,
            tenant: session.tenant,
            memberships,
            expires_at: session.expires_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use identity_domain::{Role, StoredSession, TenantId, User};
    use secrecy::ExposeSecret;
    use time::Duration;
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{FakeHasher, FakeMemberships, FakeSessions, FakeUsers, FixedClock};

    type Service = AuthService<FakeUsers, FakeMemberships, FakeSessions, FakeHasher, FixedClock>;

    struct World {
        service: Service,
        sessions: FakeSessions,
        clock: FixedClock,
    }

    fn world(users: Vec<User>) -> World {
        let sessions = FakeSessions::default();
        let clock = FixedClock::at_epoch_plus_days(20_000);
        World {
            service: AuthService::new(
                FakeUsers::with(users),
                FakeMemberships::default(),
                sessions.clone(),
                FakeHasher::default(),
                clock,
            ),
            sessions,
            clock,
        }
    }

    fn stored_for(token: &SplitToken, expires_at: OffsetDateTime) -> StoredSession {
        StoredSession {
            id: SessionId::new(Uuid::new_v4()),
            verifier_hash: token.verifier().hash(),
            user_id: UserId::new(Uuid::new_v4()),
            tenant: Some(SessionTenant {
                tenant_id: TenantId::new(Uuid::new_v4()),
                role: Role::Member,
            }),
            authenticated_at: expires_at - Duration::hours(8),
            expires_at,
        }
    }

    fn wire(token: &SplitToken) -> String {
        token.to_wire().expose_secret().to_string()
    }

    #[tokio::test]
    async fn a_valid_unexpired_session_is_accepted() -> Result<(), Box<dyn Error>> {
        let w = world(vec![]);
        let token = SplitToken::from_bytes([1; 16], [2; 32]);
        let stored = stored_for(&token, w.clock.now() + Duration::minutes(1));
        w.sessions.resolvable(token.selector(), stored.clone());

        let session = w.service.authenticate(&wire(&token)).await?;

        assert_eq!(session.id, stored.id);
        assert_eq!(session.tenant, stored.tenant);
        Ok(())
    }

    #[tokio::test]
    async fn every_rejection_is_the_same_error() -> Result<(), Box<dyn Error>> {
        let w = world(vec![]);
        let live = SplitToken::from_bytes([1; 16], [2; 32]);
        w.sessions.resolvable(
            live.selector(),
            stored_for(&live, w.clock.now() + Duration::hours(1)),
        );
        let expired = SplitToken::from_bytes([3; 16], [4; 32]);
        w.sessions
            .resolvable(expired.selector(), stored_for(&expired, w.clock.now()));
        // Right selector, wrong verifier.
        let forged = SplitToken::from_bytes([1; 16], [9; 32]);
        let unknown = SplitToken::from_bytes([7; 16], [7; 32]);

        for presented in [
            "not-a-token".to_string(),
            String::new(),
            wire(&forged),
            wire(&unknown),
            wire(&expired),
        ] {
            assert_eq!(
                w.service.authenticate(&presented).await.err(),
                Some(SessionError::Unauthenticated),
                "{presented:?}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn me_returns_the_account_for_the_session() -> Result<(), Box<dyn Error>> {
        let alice = User {
            id: UserId::new(Uuid::new_v4()),
            email: Email::parse("alice@example.test")?,
            display_name: "Alice".to_string(),
            password_hash: None,
            deactivated_at: None,
        };
        let w = world(vec![alice.clone()]);
        let session = ActiveSession {
            id: SessionId::new(Uuid::new_v4()),
            user_id: alice.id,
            tenant: None,
            authenticated_at: w.clock.now(),
            expires_at: w.clock.now() + Duration::minutes(10),
        };

        let me = w.service.me(&session).await?;

        assert_eq!(me.email, alice.email);
        assert_eq!(me.display_name, "Alice");
        assert_eq!(me.tenant, None);
        Ok(())
    }
}
