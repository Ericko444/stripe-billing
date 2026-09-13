//! In-memory fakes of the domain ports, for use-case tests. Compiled only
//! under `cfg(test)`.
//!
//! Each fake is a cheap handle over shared state, so a test keeps a clone to
//! inspect what the use case did.

use std::future::{Future, ready};
use std::sync::{Arc, Mutex, MutexGuard};

use audit::AuditEntry;
use identity_domain::{
    AccountEvent, Clock, DisplayName, Email, GrantOutcome, MailError, MailPurpose, Mailer,
    MemberGrant, MemberRepository, Membership, MembershipId, MembershipRepository,
    MembershipStatus, NewPassword, NewPasswordToken, NewSession, OutgoingMail, Password,
    PasswordHash, PasswordHashError, PasswordHasher, PasswordTokenRepository, RepositoryError,
    Selector, SessionId, SessionRepository, StoredPasswordToken, StoredSession, TenantId,
    TenantMember, TokenPurpose, User, UserId, UserRepository, Verification, VerifierHash,
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
    account_events: Arc<Mutex<Vec<(UserId, AccountEvent)>>>,
    password_changes: Arc<Mutex<Vec<(UserId, String, SessionId)>>>,
}

impl FakeUsers {
    /// Every password change: the user, the stored hash, the kept session.
    pub fn password_changes(&self) -> Vec<(UserId, String, SessionId)> {
        lock(&self.password_changes).clone()
    }

    pub fn with(users: Vec<User>) -> Self {
        Self {
            users: Arc::new(Mutex::new(users)),
            ..Self::default()
        }
    }

    /// Every account event handed to a user write, with its user.
    pub fn account_events(&self) -> Vec<(UserId, AccountEvent)> {
        lock(&self.account_events).clone()
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

    fn update_display_name(
        &self,
        user_id: UserId,
        display_name: &DisplayName,
        event: &AccountEvent,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send {
        for user in lock(&self.users).iter_mut() {
            if user.id == user_id {
                user.display_name = display_name.as_str().to_string();
            }
        }
        lock(&self.account_events).push((user_id, *event));
        ready(Ok(()))
    }

    fn change_password(
        &self,
        user_id: UserId,
        hash: &PasswordHash,
        keep: SessionId,
        events: &[AccountEvent],
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send {
        for user in lock(&self.users).iter_mut() {
            if user.id == user_id {
                user.password_hash = Some(hash.clone());
            }
        }
        lock(&self.password_changes).push((user_id, hash.as_str().to_string(), keep));
        lock(&self.account_events).extend(events.iter().map(|event| (user_id, *event)));
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

/// A token as stored, with the account event it was stored with.
pub type ReplacedToken = (NewPasswordToken, Option<AccountEvent>);

/// Every reset or invitation token stored.
#[derive(Clone, Default)]
pub struct FakeTokens {
    replaced: Arc<Mutex<Vec<ReplacedToken>>>,
    outstanding: Arc<Mutex<Vec<NewPasswordToken>>>,
    completed: Arc<Mutex<Vec<CompletedReset>>>,
}

/// A completed reset as the fake saw it: the user, the new hash, the events.
pub type CompletedReset = (UserId, String, Vec<AccountEvent>);

impl FakeTokens {
    pub fn replaced(&self) -> Vec<ReplacedToken> {
        lock(&self.replaced).clone()
    }

    pub fn completed(&self) -> Vec<CompletedReset> {
        lock(&self.completed).clone()
    }

    /// Whether a token with this selector is still outstanding.
    pub fn is_outstanding(&self, selector: Selector) -> bool {
        lock(&self.outstanding)
            .iter()
            .any(|token| token.selector == selector)
    }
}

impl PasswordTokenRepository for FakeTokens {
    fn replace(
        &self,
        token: &NewPasswordToken,
        event: Option<&AccountEvent>,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send {
        lock(&self.replaced).push((token.clone(), event.copied()));
        let mut outstanding = lock(&self.outstanding);
        outstanding.retain(|t| !(t.user_id == token.user_id && t.purpose == token.purpose));
        outstanding.push(token.clone());
        ready(Ok(()))
    }

    fn find(
        &self,
        selector: Selector,
    ) -> impl Future<Output = Result<Option<StoredPasswordToken>, RepositoryError>> + Send {
        let found = lock(&self.outstanding)
            .iter()
            .find(|token| token.selector == selector)
            .map(|token| StoredPasswordToken {
                user_id: token.user_id,
                purpose: token.purpose,
                verifier_hash: token.verifier_hash,
                expires_at: token.expires_at,
            });
        ready(Ok(found))
    }

    fn redeem(
        &self,
        selector: Selector,
        purpose: TokenPurpose,
        expected: &VerifierHash,
        now: OffsetDateTime,
        new_hash: &PasswordHash,
        events: &[AccountEvent],
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send {
        let mut outstanding = lock(&self.outstanding);
        let Some(token) = outstanding.iter().find(|t| t.selector == selector).cloned() else {
            return ready(Ok(false));
        };
        if &token.verifier_hash != expected || token.purpose != purpose || token.expires_at <= now {
            return ready(Ok(false));
        }
        outstanding.retain(|t| t.user_id != token.user_id);
        lock(&self.completed).push((
            token.user_id,
            new_hash.as_str().to_string(),
            events.to_vec(),
        ));
        ready(Ok(true))
    }
}

/// Records every grant, and answers as scripted: a new account (the
/// default), an existing one with or without a password, or an address
/// already in the tenant.
#[derive(Clone, Default)]
pub struct FakeMembers {
    grants: Arc<Mutex<Vec<MemberGrant>>>,
    existing_has_password: Option<bool>,
    already_member: bool,
}

impl FakeMembers {
    pub fn existing_account_has_password(has_password: bool) -> Self {
        Self {
            existing_has_password: Some(has_password),
            ..Self::default()
        }
    }

    pub fn already_member() -> Self {
        Self {
            already_member: true,
            ..Self::default()
        }
    }

    pub fn grants(&self) -> Vec<MemberGrant> {
        lock(&self.grants).clone()
    }
}

impl MemberRepository for FakeMembers {
    fn list(
        &self,
        _tenant_id: TenantId,
    ) -> impl Future<Output = Result<Vec<TenantMember>, RepositoryError>> + Send {
        ready(Ok(Vec::new()))
    }

    fn grant(
        &self,
        grant: &MemberGrant,
    ) -> impl Future<Output = Result<GrantOutcome, RepositoryError>> + Send {
        lock(&self.grants).push(grant.clone());
        if self.already_member {
            return ready(Ok(GrantOutcome::AlreadyMember));
        }
        ready(Ok(GrantOutcome::Granted {
            member: TenantMember {
                membership_id: MembershipId::new(Uuid::new_v4()),
                user_id: UserId::new(Uuid::new_v4()),
                email: grant.email.clone(),
                role: grant.role,
                status: MembershipStatus::Active,
            },
            invitation_issued: !self.existing_has_password.unwrap_or(false),
        }))
    }
}

/// A mail as the fake received it: recipient, purpose, the link exposed,
/// correlation id.
pub type SentMail = (Email, MailPurpose, String, audit::CorrelationId);

/// Records every mail; `failing()` refuses to send.
#[derive(Clone, Default)]
pub struct FakeMailer {
    sent: Arc<Mutex<Vec<SentMail>>>,
    fail: bool,
}

impl FakeMailer {
    pub fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }

    pub fn sent(&self) -> Vec<SentMail> {
        lock(&self.sent).clone()
    }
}

impl Mailer for FakeMailer {
    fn send(&self, mail: &OutgoingMail) -> impl Future<Output = Result<(), MailError>> + Send {
        if self.fail {
            return ready(Err(MailError("mail server unreachable".into())));
        }
        lock(&self.sent).push((
            mail.to.clone(),
            mail.purpose,
            secrecy::ExposeSecret::expose_secret(&mail.link).to_string(),
            mail.correlation_id,
        ));
        ready(Ok(()))
    }
}

impl FixedClock {
    /// A clock stopped at `instant`.
    pub fn at(instant: OffsetDateTime) -> Self {
        Self(instant)
    }
}
