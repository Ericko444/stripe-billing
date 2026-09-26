use audit::{Action, Actor, AuditEntry, CorrelationId, SubjectId, Target, TargetId};
use identity_domain::{
    Clock, Email, Membership, MembershipRepository, NewPassword, NewSession, Password,
    PasswordHasher, SessionRepository, SplitToken, UserId, UserRepository, Verification,
};
use thiserror::Error;
use time::{Duration, OffsetDateTime};

use crate::token::generate_token;

/// How long a tenant-scoped session is accepted, measured from the moment
/// the password was proven. Absolute: nothing extends it, including
/// switching tenant. No sliding window -- that would write on every request.
pub const TENANT_SESSION_LIFETIME: Duration = Duration::hours(8);

/// How long the tenant-less session lives. It exists only so a user with
/// several memberships can pick one without re-entering a password.
pub const UNSCOPED_SESSION_LIFETIME: Duration = Duration::minutes(10);

/// Login, and (as they land) the other session use cases.
pub struct AuthService<U, M, S, H, C> {
    pub(crate) users: U,
    pub(crate) memberships: M,
    pub(crate) sessions: S,
    pub(crate) hasher: H,
    pub(crate) clock: C,
}

/// What a session issued at login is scoped to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionScope {
    /// Scoped to the user's only active membership.
    Tenant(Membership),
    /// Not scoped: the user has several memberships and must pick one.
    Unscoped,
}

/// A successful login.
#[derive(Debug)]
pub struct LoginOutcome {
    /// The credential to hand the client, once, in a cookie. The server
    /// keeps only its selector and verifier hash.
    pub token: SplitToken,
    /// Who logged in.
    pub user_id: UserId,
    /// What the session is scoped to.
    pub scope: SessionScope,
    /// Every active membership, for a tenant picker.
    pub memberships: Vec<Membership>,
    /// When the session stops being accepted.
    pub expires_at: OffsetDateTime,
}

/// Why a login did not produce a session.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LoginError {
    /// Unknown address, wrong password, deactivated account, an invited
    /// account with no password yet, or no active membership anywhere --
    /// **one variant for all five**, so nothing downstream can tell a caller
    /// which it was.
    #[error("invalid credentials")]
    InvalidCredentials,
    /// Something this module depends on failed. The message is for logs.
    #[error("login unavailable: {0}")]
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
    /// Builds the service over its ports.
    pub fn new(users: U, memberships: M, sessions: S, hasher: H, clock: C) -> Self {
        Self {
            users,
            memberships,
            sessions,
            hasher,
            clock,
        }
    }

    /// Verifies `password` for `email` and issues a session.
    ///
    /// Exactly one password verification runs on every path. When there is
    /// no usable stored hash -- no account, a deactivated one, or an invited
    /// one with no password -- it runs against the hasher's dummy hash, so
    /// the Argon2id cost is paid either way and response time does not say
    /// whether the address is registered.
    pub async fn login(
        &self,
        email: &Email,
        password: &Password,
        correlation_id: CorrelationId,
    ) -> Result<LoginOutcome, LoginError> {
        let user = self.users.find_by_email(email).await.map_err(unavailable)?;
        let candidate = user
            .filter(|user| user.is_active())
            .and_then(|user| user.password_hash.clone().map(|hash| (user, hash)));

        let stored = match &candidate {
            Some((_, hash)) => hash,
            None => self.hasher.dummy_hash(),
        };
        let verification = self
            .hasher
            .verify(password, stored)
            .await
            .map_err(unavailable)?;

        let (user, _) = candidate.ok_or(LoginError::InvalidCredentials)?;
        if verification == Verification::Mismatch {
            return Err(LoginError::InvalidCredentials);
        }

        let memberships = self
            .memberships
            .active_for_user(user.id)
            .await
            .map_err(unavailable)?;
        let scope = match memberships.as_slice() {
            [] => return Err(LoginError::InvalidCredentials),
            [only] => SessionScope::Tenant(only.clone()),
            _ => SessionScope::Unscoped,
        };

        if verification == Verification::MatchNeedsRehash {
            self.rehash(user.id, password).await;
        }

        let now = self.clock.now();
        let token = generate_token().map_err(unavailable)?;
        let (tenant_id, lifetime, audit) = match &scope {
            SessionScope::Tenant(membership) => (
                Some(membership.tenant_id),
                TENANT_SESSION_LIFETIME,
                Some(session_started(membership, now, correlation_id)),
            ),
            SessionScope::Unscoped => (None, UNSCOPED_SESSION_LIFETIME, None),
        };
        let session = NewSession {
            selector: token.selector(),
            verifier_hash: token.verifier().hash(),
            user_id: user.id,
            tenant_id,
            authenticated_at: now,
            expires_at: now + lifetime,
        };
        self.sessions
            .create(&session, audit.as_ref())
            .await
            .map_err(unavailable)?;

        Ok(LoginOutcome {
            token,
            user_id: user.id,
            scope,
            memberships,
            expires_at: session.expires_at,
        })
    }

    /// Stores a hash made under the live parameters, best effort. A password
    /// set under an older, looser policy cannot become a `NewPassword`, and
    /// any failure here must not fail a login that already succeeded -- in
    /// both cases the old hash simply stays until the next password change.
    async fn rehash(&self, user_id: UserId, password: &Password) {
        let Ok(checked) = NewPassword::check(Password::new(password.expose_secret().to_string()))
        else {
            return;
        };
        if let Ok(hash) = self.hasher.hash(&checked).await {
            let _ = self.users.rehash_password(user_id, &hash).await;
        }
    }
}

/// The audit entry for a tenant-scoped session starting: the user acted on
/// themselves, within the tenant the session is scoped to.
pub(crate) fn session_started(
    membership: &Membership,
    occurred_at: OffsetDateTime,
    correlation_id: CorrelationId,
) -> AuditEntry {
    let user = membership.user_id.as_uuid();
    AuditEntry::new(
        audit::TenantId::new(membership.tenant_id.as_uuid()),
        Actor::User(SubjectId::new(user)),
        Action::SessionStarted,
        Target::User(TargetId::new(user)),
        occurred_at,
        correlation_id,
    )
}

fn unavailable(err: impl std::fmt::Display) -> LoginError {
    LoginError::Unavailable(err.to_string())
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use identity_domain::{MembershipId, PasswordHash, Role, TenantId, User};
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{
        FakeHasher, FakeMemberships, FakeSessions, FakeUsers, FixedClock, STALE_PREFIX,
    };

    const PASSWORD: &str = "correct horse battery staple";

    fn email(raw: &str) -> Result<Email, Box<dyn Error>> {
        Ok(Email::parse(raw)?)
    }

    fn password(raw: &str) -> Password {
        Password::new(raw.to_string())
    }

    fn user(address: &str, hash: Option<String>) -> Result<User, Box<dyn Error>> {
        Ok(User {
            id: UserId::new(Uuid::new_v4()),
            email: email(address)?,
            display_name: String::new(),
            password_hash: hash.map(PasswordHash::new),
            deactivated_at: None,
        })
    }

    fn membership_of(user: &User, name: &str, role: Role) -> Membership {
        Membership {
            id: MembershipId::new(Uuid::new_v4()),
            user_id: user.id,
            tenant_id: TenantId::new(Uuid::new_v4()),
            tenant_name: name.to_string(),
            role,
        }
    }

    struct World {
        service: AuthService<FakeUsers, FakeMemberships, FakeSessions, FakeHasher, FixedClock>,
        users: FakeUsers,
        sessions: FakeSessions,
        hasher: FakeHasher,
        clock: FixedClock,
    }

    fn world(users: Vec<User>, memberships: Vec<Membership>) -> World {
        let users = FakeUsers::with(users);
        let memberships = FakeMemberships::with(memberships);
        let sessions = FakeSessions::default();
        let hasher = FakeHasher::default();
        let clock = FixedClock::at_epoch_plus_days(20_000);
        World {
            service: AuthService::new(
                users.clone(),
                memberships,
                sessions.clone(),
                hasher.clone(),
                clock,
            ),
            users,
            sessions,
            hasher,
            clock,
        }
    }

    fn correlation() -> CorrelationId {
        CorrelationId::new(Uuid::new_v4())
    }

    #[tokio::test]
    async fn every_credential_failure_is_the_same_error() -> Result<(), Box<dyn Error>> {
        let active = user("alice@example.test", Some(FakeHasher::hash_of(PASSWORD)))?;
        let mut deactivated = user("dave@example.test", Some(FakeHasher::hash_of(PASSWORD)))?;
        deactivated.deactivated_at = Some(OffsetDateTime::UNIX_EPOCH);
        let invited = user("ivy@example.test", None)?;
        let no_membership = user("nora@example.test", Some(FakeHasher::hash_of(PASSWORD)))?;
        let memberships = vec![
            membership_of(&active, "Tenant A", Role::Owner),
            membership_of(&deactivated, "Tenant A", Role::Member),
            membership_of(&invited, "Tenant A", Role::Member),
        ];
        let w = world(
            vec![active, deactivated, invited, no_membership],
            memberships,
        );

        for (address, attempt) in [
            ("nobody@example.test", PASSWORD),
            ("alice@example.test", "wrong horse battery staple"),
            ("dave@example.test", PASSWORD),
            ("ivy@example.test", PASSWORD),
            ("nora@example.test", PASSWORD),
        ] {
            let result = w
                .service
                .login(&email(address)?, &password(attempt), correlation())
                .await;
            assert_eq!(
                result.err(),
                Some(LoginError::InvalidCredentials),
                "{address}"
            );
        }
        assert!(w.sessions.stored().is_empty());
        Ok(())
    }

    /// The timing half of "no enumeration" at login: every path runs
    /// exactly one verification, and the paths with no usable stored hash
    /// run it against the dummy.
    #[tokio::test]
    async fn accounts_without_a_usable_hash_verify_against_the_dummy() -> Result<(), Box<dyn Error>>
    {
        let mut deactivated = user("dave@example.test", Some(FakeHasher::hash_of(PASSWORD)))?;
        deactivated.deactivated_at = Some(OffsetDateTime::UNIX_EPOCH);
        let invited = user("ivy@example.test", None)?;
        let w = world(vec![deactivated, invited], vec![]);

        for address in [
            "nobody@example.test",
            "dave@example.test",
            "ivy@example.test",
        ] {
            let _ = w
                .service
                .login(&email(address)?, &password(PASSWORD), correlation())
                .await;
        }

        assert_eq!(
            w.hasher.verified_against(),
            vec![FakeHasher::DUMMY.to_string(); 3]
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_single_membership_gets_an_eight_hour_tenant_session_and_an_audit_entry()
    -> Result<(), Box<dyn Error>> {
        let alice = user("alice@example.test", Some(FakeHasher::hash_of(PASSWORD)))?;
        let tenant_a = membership_of(&alice, "Tenant A", Role::Owner);
        let w = world(vec![alice.clone()], vec![tenant_a.clone()]);
        let correlation_id = correlation();

        let outcome = w
            .service
            .login(
                &email("alice@example.test")?,
                &password(PASSWORD),
                correlation_id,
            )
            .await?;

        assert_eq!(outcome.scope, SessionScope::Tenant(tenant_a.clone()));
        let stored = w.sessions.stored();
        let [(session, Some(entry))] = stored.as_slice() else {
            return Err("expected one audited session".into());
        };
        assert_eq!(session.tenant_id, Some(tenant_a.tenant_id));
        assert_eq!(session.authenticated_at, w.clock.now());
        assert_eq!(session.expires_at, w.clock.now() + Duration::hours(8));
        assert!(session.verifier_hash.verifies(outcome.token.verifier()));
        assert_eq!(session.selector, outcome.token.selector());

        assert_eq!(entry.tenant_id().as_uuid(), tenant_a.tenant_id.as_uuid());
        assert_eq!(
            entry.actor(),
            Actor::User(SubjectId::new(alice.id.as_uuid()))
        );
        assert_eq!(entry.action(), Action::SessionStarted);
        assert_eq!(
            entry.target(),
            Target::User(TargetId::new(alice.id.as_uuid()))
        );
        assert_eq!(entry.occurred_at(), w.clock.now());
        assert_eq!(entry.correlation_id(), correlation_id);
        Ok(())
    }

    #[tokio::test]
    async fn several_memberships_get_a_ten_minute_unscoped_session_and_no_audit_entry()
    -> Result<(), Box<dyn Error>> {
        let alice = user("alice@example.test", Some(FakeHasher::hash_of(PASSWORD)))?;
        let memberships = vec![
            membership_of(&alice, "Tenant A", Role::Owner),
            membership_of(&alice, "Tenant B", Role::Member),
        ];
        let w = world(vec![alice], memberships.clone());

        let outcome = w
            .service
            .login(
                &email("alice@example.test")?,
                &password(PASSWORD),
                correlation(),
            )
            .await?;

        assert_eq!(outcome.scope, SessionScope::Unscoped);
        assert_eq!(outcome.memberships, memberships);
        let stored = w.sessions.stored();
        let [(session, None)] = stored.as_slice() else {
            return Err("expected one unaudited session".into());
        };
        assert_eq!(session.tenant_id, None);
        assert_eq!(session.expires_at, w.clock.now() + Duration::minutes(10));
        Ok(())
    }

    #[tokio::test]
    async fn a_stale_hash_is_replaced_after_a_successful_login() -> Result<(), Box<dyn Error>> {
        let alice = user(
            "alice@example.test",
            Some(format!("{STALE_PREFIX}{PASSWORD}")),
        )?;
        let alice_id = alice.id;
        let tenant = membership_of(&alice, "Tenant A", Role::Owner);
        let w = world(vec![alice], vec![tenant]);

        w.service
            .login(
                &email("alice@example.test")?,
                &password(PASSWORD),
                correlation(),
            )
            .await?;

        assert_eq!(
            w.users.rehashed(),
            vec![(alice_id, FakeHasher::hash_of(PASSWORD))]
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_stale_hash_under_an_older_policy_still_logs_in_without_rehash()
    -> Result<(), Box<dyn Error>> {
        let short = "short but old";
        let alice = user("alice@example.test", Some(format!("{STALE_PREFIX}{short}")))?;
        let tenant = membership_of(&alice, "Tenant A", Role::Owner);
        let w = world(vec![alice], vec![tenant]);

        let outcome = w
            .service
            .login(
                &email("alice@example.test")?,
                &password(short),
                correlation(),
            )
            .await;

        assert!(outcome.is_ok());
        assert!(w.users.rehashed().is_empty());
        Ok(())
    }
}
