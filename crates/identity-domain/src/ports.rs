//! Ports: what the identity use cases need from the outside world, with no
//! opinion on how it is provided.
//!
//! Methods are written `fn … -> impl Future<Output = …> + Send` rather than
//! bare `async fn`, for the reason `domain`'s repository ports give: a use
//! case generic over a port is later erased behind a `dyn`-safe façade for
//! the router, and that needs `Send` promised on the trait itself.

use std::future::Future;

use audit::AuditEntry;
use thiserror::Error;
use time::OffsetDateTime;

use crate::{
    AccountEvent, DisplayName, Email, Membership, NewPassword, NewSession, Password, PasswordHash,
    Selector, SessionId, StoredSession, User, UserId,
};

/// Finds and updates users.
pub trait UserRepository: Send + Sync {
    /// The user with this normalised address, active or not. Deactivation
    /// is the caller's decision to act on, so that it can be handled on the
    /// same code path -- and at the same cost -- as an unknown address.
    fn find_by_email(
        &self,
        email: &Email,
    ) -> impl Future<Output = Result<Option<User>, RepositoryError>> + Send;

    /// The user with this id, active or not.
    fn find(
        &self,
        user_id: UserId,
    ) -> impl Future<Output = Result<Option<User>, RepositoryError>> + Send;

    /// Replaces a user's password hash with one made under newer
    /// parameters, after a successful login. Not audited: the password did
    /// not change, only how it is stored.
    fn rehash_password(
        &self,
        user_id: UserId,
        hash: &PasswordHash,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send;

    /// Sets a user's display name, and records `event` once per active
    /// membership, in one transaction.
    fn update_display_name(
        &self,
        user_id: UserId,
        display_name: &DisplayName,
        event: &AccountEvent,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send;

    /// Replaces a user's password after they proved the current one, in one
    /// transaction: stores `hash`, deletes **every other** session of the
    /// user in every tenant (keeping `keep`, the one that made the change),
    /// deletes any outstanding reset or invitation token, and records each of
    /// `events` once per active membership.
    fn change_password(
        &self,
        user_id: UserId,
        hash: &PasswordHash,
        keep: SessionId,
        events: &[AccountEvent],
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send;
}

/// Reads memberships.
pub trait MembershipRepository: Send + Sync {
    /// Every **active** membership of `user_id`, in tenants that exist,
    /// ordered by tenant name.
    fn active_for_user(
        &self,
        user_id: UserId,
    ) -> impl Future<Output = Result<Vec<Membership>, RepositoryError>> + Send;
}

/// Stores sessions.
pub trait SessionRepository: Send + Sync {
    /// Stores `session`. When `audit` is given, it is written in the same
    /// transaction, so a session and the record of it starting commit or
    /// fail together -- the pattern the billing module's repositories use.
    fn create(
        &self,
        session: &NewSession,
        audit: Option<&AuditEntry>,
    ) -> impl Future<Output = Result<SessionId, RepositoryError>> + Send;

    /// The session with `selector`, if it exists **and** its user is active
    /// **and**, when it is tenant-scoped, its membership is active. One
    /// query, run on every authenticated request, so a suspension or a
    /// deactivation takes effect on the next request rather than at expiry.
    ///
    /// Expired sessions are still returned: expiry is checked by the caller
    /// against the module's one clock.
    fn resolve(
        &self,
        selector: Selector,
    ) -> impl Future<Output = Result<Option<StoredSession>, RepositoryError>> + Send;

    /// Replaces session `old` with `new`, and writes `audit`, in one
    /// transaction. Returns `None` -- and changes nothing -- if `old` no
    /// longer exists, so two concurrent rotations of one token cannot both
    /// succeed and leave two live sessions behind.
    fn rotate(
        &self,
        old: SessionId,
        new: &NewSession,
        audit: Option<&AuditEntry>,
    ) -> impl Future<Output = Result<Option<SessionId>, RepositoryError>> + Send;

    /// Deletes session `id`, if it still exists.
    fn delete(&self, id: SessionId) -> impl Future<Output = Result<(), RepositoryError>> + Send;
}

/// The one clock the module reads.
///
/// Every issuance and every expiry check goes through this, so the two can
/// never disagree about what time it is -- and tests can say what time it
/// is. The database's `now()` is deliberately not used for either.
pub trait Clock: Send + Sync {
    /// The current instant, in UTC.
    fn now(&self) -> OffsetDateTime;
}

/// A persistence operation failed. Opaque for the reason
/// `domain::DomainError::Repository` is: this crate must not know `sqlx`'s
/// error type, so the adapter renders its own into the message.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("repository error: {0}")]
pub struct RepositoryError(pub String);

/// Hashes and verifies passwords. Implemented with Argon2id in
/// `identity-service`; use-case tests use a fast fake.
pub trait PasswordHasher: Send + Sync {
    /// Hashes a password that has passed the policy. Taking
    /// [`NewPassword`] rather than [`Password`] is what makes "stored a
    /// password nobody checked" a type error.
    fn hash(
        &self,
        password: &NewPassword,
    ) -> impl Future<Output = Result<PasswordHash, PasswordHashError>> + Send;

    /// Verifies `password` against `stored`, using the parameters recorded
    /// in `stored` -- so a hash made under older parameters still verifies,
    /// and reports that it should be replaced.
    fn verify(
        &self,
        password: &Password,
        stored: &PasswordHash,
    ) -> impl Future<Output = Result<Verification, PasswordHashError>> + Send;

    /// A hash, made with the live parameters, of a password no one knows.
    ///
    /// Login verifies against this when the address has no account (or the
    /// account has no password yet), so the unknown-address path pays the
    /// same hashing cost, through the same `verify` call, as the
    /// known-address path. Without it, response time alone would say
    /// whether an address is registered.
    fn dummy_hash(&self) -> &PasswordHash;
}

/// The outcome of a verification that ran to completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verification {
    /// The password does not match.
    Mismatch,
    /// The password matches, and the stored hash uses the live parameters.
    Match,
    /// The password matches, but the stored hash was made with different
    /// parameters or algorithm; the caller should store a fresh hash while
    /// it holds the plaintext.
    MatchNeedsRehash,
}

/// Hashing or verification could not run -- a malformed stored hash, an
/// exhausted worker pool, a panicked blocking task. Never "wrong password":
/// that is [`Verification::Mismatch`]. The message is for server logs only
/// and carries no password material.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("password hashing failed: {0}")]
pub struct PasswordHashError(pub String);
