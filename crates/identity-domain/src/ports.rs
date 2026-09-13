//! Ports: what the identity use cases need from the outside world, with no
//! opinion on how it is provided.
//!
//! Methods are written `fn … -> impl Future<Output = …> + Send` rather than
//! bare `async fn`, for the reason `domain`'s repository ports give: a use
//! case generic over a port is later erased behind a `dyn`-safe façade for
//! the router, and that needs `Send` promised on the trait itself.

use std::future::Future;

use thiserror::Error;

use crate::{NewPassword, Password, PasswordHash};

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
