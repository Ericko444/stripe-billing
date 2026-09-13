use audit::CorrelationId;
use identity_domain::{
    Clock, Email, Membership, MembershipRepository, NewSession, PasswordHasher, SessionId,
    SessionRepository, SessionTenant, SplitToken, TenantId, UserId, UserRepository,
};
use thiserror::Error;
use time::{Duration, OffsetDateTime};

use crate::auth::session_started;
use crate::token::generate_token;
use crate::{AuthService, TENANT_SESSION_LIFETIME};

/// A tenant picked for a session: the rotated token and what it now grants.
#[derive(Debug)]
pub struct TenantSelection {
    /// The new credential. The one it replaces no longer works.
    pub token: SplitToken,
    /// The membership the session is now scoped to.
    pub membership: Membership,
    /// Every active membership, for the tenant switcher.
    pub memberships: Vec<Membership>,
    /// When the session stops being accepted -- unchanged by switching.
    pub expires_at: OffsetDateTime,
    /// How long that is from now, for the cookie's `Max-Age`.
    pub max_age: Duration,
}

/// Why a tenant could not be picked.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SelectTenantError {
    /// The caller has no active membership in that tenant -- or the tenant
    /// does not exist. One variant for both, so the answer does not confirm
    /// that some tenant id is real.
    #[error("not a member of that tenant")]
    NotAMember,
    /// The session ended while the request was in flight: it expired, or a
    /// concurrent request already rotated or logged it out.
    #[error("not authenticated")]
    Unauthenticated,
    /// Something this module depends on failed. The message is for logs.
    #[error("tenant selection unavailable: {0}")]
    Unavailable(String),
}

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

    /// Scopes the caller's session to `tenant_id`, whether it was tenant-less
    /// or scoped to another tenant.
    ///
    /// The token is **rotated**: a new one is issued and the old row deleted
    /// in the same transaction as the `SessionStarted` audit entry. A token
    /// that existed before the caller gained access to a tenant never grants
    /// it -- the session-fixation defence -- and a switch leaves exactly one
    /// live session. `authenticated_at` is carried over, so the new session
    /// expires eight hours after the password was proven, not eight hours
    /// after the switch: switching never extends a session.
    pub async fn select_tenant(
        &self,
        session: &ActiveSession,
        tenant_id: TenantId,
        correlation_id: CorrelationId,
    ) -> Result<TenantSelection, SelectTenantError> {
        let unavailable =
            |err: identity_domain::RepositoryError| SelectTenantError::Unavailable(err.to_string());
        let memberships = self
            .memberships
            .active_for_user(session.user_id)
            .await
            .map_err(unavailable)?;
        let membership = memberships
            .iter()
            .find(|membership| membership.tenant_id == tenant_id)
            .cloned()
            .ok_or(SelectTenantError::NotAMember)?;

        let now = self.clock.now();
        let expires_at = session.authenticated_at + TENANT_SESSION_LIFETIME;
        if expires_at <= now {
            return Err(SelectTenantError::Unauthenticated);
        }

        let token =
            generate_token().map_err(|err| SelectTenantError::Unavailable(err.to_string()))?;
        let replacement = NewSession {
            selector: token.selector(),
            verifier_hash: token.verifier().hash(),
            user_id: session.user_id,
            tenant_id: Some(tenant_id),
            authenticated_at: session.authenticated_at,
            expires_at,
        };
        let audit = session_started(&membership, now, correlation_id);
        self.sessions
            .rotate(session.id, &replacement, Some(&audit))
            .await
            .map_err(unavailable)?
            .ok_or(SelectTenantError::Unauthenticated)?;

        Ok(TenantSelection {
            token,
            membership,
            memberships,
            expires_at,
            max_age: expires_at - now,
        })
    }

    /// Ends the caller's session. Not audited: ending one's own session
    /// changes nothing another party needs a record of.
    pub async fn logout(&self, session: &ActiveSession) -> Result<(), SessionError> {
        self.sessions
            .delete(session.id)
            .await
            .map_err(|err| SessionError::Unavailable(err.to_string()))
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
        world_with(users, vec![])
    }

    fn world_with(users: Vec<User>, memberships: Vec<Membership>) -> World {
        let sessions = FakeSessions::default();
        let clock = FixedClock::at_epoch_plus_days(20_000);
        World {
            service: AuthService::new(
                FakeUsers::with(users),
                FakeMemberships::with(memberships),
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

    fn membership_in(user: UserId, name: &str) -> Membership {
        Membership {
            id: identity_domain::MembershipId::new(Uuid::new_v4()),
            user_id: user,
            tenant_id: TenantId::new(Uuid::new_v4()),
            tenant_name: name.to_string(),
            role: Role::Member,
        }
    }

    fn session_of(user: UserId, authenticated_at: OffsetDateTime) -> ActiveSession {
        ActiveSession {
            id: SessionId::new(Uuid::new_v4()),
            user_id: user,
            tenant: None,
            authenticated_at,
            expires_at: authenticated_at + Duration::minutes(10),
        }
    }

    #[tokio::test]
    async fn selecting_a_tenant_rotates_the_token_and_audits_the_start()
    -> Result<(), Box<dyn Error>> {
        let alice = UserId::new(Uuid::new_v4());
        let tenant_b = membership_in(alice, "Tenant B");
        let w = world_with(
            vec![],
            vec![membership_in(alice, "Tenant A"), tenant_b.clone()],
        );
        let session = session_of(alice, w.clock.now());
        let correlation_id = CorrelationId::new(Uuid::new_v4());

        let selection = w
            .service
            .select_tenant(&session, tenant_b.tenant_id, correlation_id)
            .await?;

        let rotated = w.sessions.rotated();
        let [(old, (replacement, Some(entry)))] = rotated.as_slice() else {
            return Err("expected one audited rotation".into());
        };
        assert_eq!(*old, session.id);
        assert_eq!(replacement.tenant_id, Some(tenant_b.tenant_id));
        assert_eq!(replacement.selector, selection.token.selector());
        assert!(
            replacement
                .verifier_hash
                .verifies(selection.token.verifier())
        );
        assert_eq!(entry.tenant_id().as_uuid(), tenant_b.tenant_id.as_uuid());
        assert_eq!(entry.action(), audit::Action::SessionStarted);
        assert_eq!(entry.correlation_id(), correlation_id);
        assert_eq!(selection.membership, tenant_b);
        Ok(())
    }

    /// Selecting at hour seven of an eight-hour session yields a session
    /// that ends at hour eight -- not one that starts eight hours over.
    #[tokio::test]
    async fn switching_tenant_never_extends_the_absolute_lifetime() -> Result<(), Box<dyn Error>> {
        let alice = UserId::new(Uuid::new_v4());
        let tenant_a = membership_in(alice, "Tenant A");
        let w = world_with(vec![], vec![tenant_a.clone()]);
        let authenticated_at = w.clock.now() - Duration::hours(7);
        let session = session_of(alice, authenticated_at);

        let selection = w
            .service
            .select_tenant(
                &session,
                tenant_a.tenant_id,
                CorrelationId::new(Uuid::new_v4()),
            )
            .await?;

        assert_eq!(selection.expires_at, authenticated_at + Duration::hours(8));
        assert_eq!(selection.max_age, Duration::hours(1));
        let rotated = w.sessions.rotated();
        assert!(matches!(
            rotated.as_slice(),
            [(_, (replacement, _))] if replacement.authenticated_at == authenticated_at
        ));
        Ok(())
    }

    #[tokio::test]
    async fn a_tenant_without_an_active_membership_is_not_a_member() -> Result<(), Box<dyn Error>> {
        let alice = UserId::new(Uuid::new_v4());
        let someone_elses = membership_in(UserId::new(Uuid::new_v4()), "Tenant X");
        let w = world_with(
            vec![],
            vec![membership_in(alice, "Tenant A"), someone_elses.clone()],
        );
        let session = session_of(alice, w.clock.now());

        for tenant in [someone_elses.tenant_id, TenantId::new(Uuid::new_v4())] {
            let result = w
                .service
                .select_tenant(&session, tenant, CorrelationId::new(Uuid::new_v4()))
                .await;
            assert_eq!(result.err(), Some(SelectTenantError::NotAMember));
        }
        assert!(w.sessions.rotated().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn a_session_already_rotated_elsewhere_cannot_be_rotated_again()
    -> Result<(), Box<dyn Error>> {
        let alice = UserId::new(Uuid::new_v4());
        let tenant_a = membership_in(alice, "Tenant A");
        let w = world_with(vec![], vec![tenant_a.clone()]);
        let session = session_of(alice, w.clock.now());
        w.sessions.already_gone(session.id);

        let result = w
            .service
            .select_tenant(
                &session,
                tenant_a.tenant_id,
                CorrelationId::new(Uuid::new_v4()),
            )
            .await;

        assert_eq!(result.err(), Some(SelectTenantError::Unauthenticated));
        Ok(())
    }

    #[tokio::test]
    async fn logout_deletes_the_session() -> Result<(), Box<dyn Error>> {
        let w = world(vec![]);
        let session = session_of(UserId::new(Uuid::new_v4()), w.clock.now());

        w.service.logout(&session).await?;

        assert_eq!(w.sessions.deleted(), vec![session.id]);
        Ok(())
    }
}
