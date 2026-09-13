//! In-memory fakes of the domain ports, for use-case tests. Compiled only
//! under `cfg(test)`.
//!
//! Each fake is a cheap handle over shared state, so a test keeps a clone to
//! inspect what the use case did.

use std::future::{Future, ready};
use std::sync::{Arc, Mutex, MutexGuard};

use audit::AuditEntry;
use identity_domain::{
    Clock, Email, Membership, MembershipRepository, NewPassword, NewSession, Password,
    PasswordHash, PasswordHashError, PasswordHasher, RepositoryError, Selector, SessionId,
    SessionRepository, StoredSession, User, UserId, UserRepository, Verification,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Locks a fake's state, recovering from a poisoned lock -- a panicking
/// test has already failed, and the workspace denies `unwrap`.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Users, and a record of rehashes.
#[derive(Clone, Default)]
pub struct FakeUsers {
    users: Arc<Mutex<Vec<User>>>,
    rehashed: Arc<Mutex<Vec<(UserId, String)>>>,
}

impl FakeUsers {
    pub fn with(users: Vec<User>) -> Self {
        Self {
            users: Arc::new(Mutex::new(users)),
            rehashed: Arc::default(),
        }
    }

    pub fn rehashed(&self) -> Vec<(UserId, String)> {
        lock(&self.rehashed).clone()
    }
}

impl UserRepository for FakeUsers {
    fn find_by_email(
        &self,
        email: &Email,
    ) -> impl Future<Output = Result<Option<User>, RepositoryError>> + Send {
        let found = lock(&self.users)
            .iter()
            .find(|user| &user.email == email)
            .cloned();
        ready(Ok(found))
    }

    fn find(
        &self,
        user_id: UserId,
    ) -> impl Future<Output = Result<Option<User>, RepositoryError>> + Send {
        let found = lock(&self.users)
            .iter()
            .find(|user| user.id == user_id)
            .cloned();
        ready(Ok(found))
    }

    fn rehash_password(
        &self,
        user_id: UserId,
        hash: &PasswordHash,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send {
        lock(&self.rehashed).push((user_id, hash.as_str().to_string()));
        ready(Ok(()))
    }
}

/// Active memberships.
#[derive(Clone, Default)]
pub struct FakeMemberships {
    memberships: Arc<Mutex<Vec<Membership>>>,
}

impl FakeMemberships {
    pub fn with(memberships: Vec<Membership>) -> Self {
        Self {
            memberships: Arc::new(Mutex::new(memberships)),
        }
    }
}

impl MembershipRepository for FakeMemberships {
    fn active_for_user(
        &self,
        user_id: UserId,
    ) -> impl Future<Output = Result<Vec<Membership>, RepositoryError>> + Send {
        let found = lock(&self.memberships)
            .iter()
            .filter(|membership| membership.user_id == user_id)
            .cloned()
            .collect();
        ready(Ok(found))
    }
}

/// A session as created, with the audit entry it was created with.
pub type CreatedSession = (NewSession, Option<AuditEntry>);

/// Every session created, with the audit entry it was created with.
#[derive(Clone, Default)]
pub struct FakeSessions {
    stored: Arc<Mutex<Vec<CreatedSession>>>,
    resolvable: Arc<Mutex<Vec<(Selector, StoredSession)>>>,
    rotated: Arc<Mutex<Vec<(SessionId, CreatedSession)>>>,
    gone: Arc<Mutex<Vec<SessionId>>>,
    deleted: Arc<Mutex<Vec<SessionId>>>,
}

impl FakeSessions {
    /// Makes a later `rotate` of `id` find nothing, as if another request
    /// had rotated or deleted it first.
    pub fn already_gone(&self, id: SessionId) {
        lock(&self.gone).push(id);
    }

    pub fn rotated(&self) -> Vec<(SessionId, CreatedSession)> {
        lock(&self.rotated).clone()
    }

    pub fn deleted(&self) -> Vec<SessionId> {
        lock(&self.deleted).clone()
    }

    /// Makes `session` findable by `selector`, as if the lookup query had
    /// matched it -- active user, active membership.
    pub fn resolvable(&self, selector: Selector, session: StoredSession) {
        lock(&self.resolvable).push((selector, session));
    }

    pub fn stored(&self) -> Vec<CreatedSession> {
        lock(&self.stored).clone()
    }
}

impl SessionRepository for FakeSessions {
    fn create(
        &self,
        session: &NewSession,
        audit: Option<&AuditEntry>,
    ) -> impl Future<Output = Result<SessionId, RepositoryError>> + Send {
        lock(&self.stored).push((session.clone(), audit.cloned()));
        ready(Ok(SessionId::new(Uuid::new_v4())))
    }

    fn resolve(
        &self,
        selector: Selector,
    ) -> impl Future<Output = Result<Option<StoredSession>, RepositoryError>> + Send {
        let found = lock(&self.resolvable)
            .iter()
            .find(|(candidate, _)| *candidate == selector)
            .map(|(_, session)| session.clone());
        ready(Ok(found))
    }

    fn rotate(
        &self,
        old: SessionId,
        new: &NewSession,
        audit: Option<&AuditEntry>,
    ) -> impl Future<Output = Result<Option<SessionId>, RepositoryError>> + Send {
        if lock(&self.gone).contains(&old) {
            return ready(Ok(None));
        }
        lock(&self.rotated).push((old, (new.clone(), audit.cloned())));
        ready(Ok(Some(SessionId::new(Uuid::new_v4()))))
    }

    fn delete(&self, id: SessionId) -> impl Future<Output = Result<(), RepositoryError>> + Send {
        lock(&self.deleted).push(id);
        ready(Ok(()))
    }
}

/// A stored hash with this prefix verifies, but reports it needs a rehash.
pub const STALE_PREFIX: &str = "stale:";

/// A transparent "hasher": the hash of `p` is `fake:p`. Records every hash
/// it was asked to verify against.
#[derive(Clone)]
pub struct FakeHasher {
    dummy: PasswordHash,
    verified_against: Arc<Mutex<Vec<String>>>,
}

impl FakeHasher {
    pub const DUMMY: &'static str = "fake-dummy";

    pub fn hash_of(password: &str) -> String {
        format!("fake:{password}")
    }

    pub fn verified_against(&self) -> Vec<String> {
        lock(&self.verified_against).clone()
    }
}

impl Default for FakeHasher {
    fn default() -> Self {
        Self {
            dummy: PasswordHash::new(Self::DUMMY.to_string()),
            verified_against: Arc::default(),
        }
    }
}

impl PasswordHasher for FakeHasher {
    fn hash(
        &self,
        password: &NewPassword,
    ) -> impl Future<Output = Result<PasswordHash, PasswordHashError>> + Send {
        let hash = Self::hash_of(password.as_password().expose_secret());
        ready(Ok(PasswordHash::new(hash)))
    }

    fn verify(
        &self,
        password: &Password,
        stored: &PasswordHash,
    ) -> impl Future<Output = Result<Verification, PasswordHashError>> + Send {
        lock(&self.verified_against).push(stored.as_str().to_string());
        let presented = password.expose_secret();
        let outcome = if stored.as_str() == Self::hash_of(presented) {
            Verification::Match
        } else if stored.as_str() == format!("{STALE_PREFIX}{presented}") {
            Verification::MatchNeedsRehash
        } else {
            Verification::Mismatch
        };
        ready(Ok(outcome))
    }

    fn dummy_hash(&self) -> &PasswordHash {
        &self.dummy
    }
}

/// A clock that always says the same instant.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(OffsetDateTime);

impl FixedClock {
    pub fn at_epoch_plus_days(days: i64) -> Self {
        Self(OffsetDateTime::UNIX_EPOCH + Duration::days(days))
    }
}

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.0
    }
}
