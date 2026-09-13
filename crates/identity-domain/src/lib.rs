//! The identity module's domain: users belonging to several tenants, one
//! role per membership, password authentication and password reset.
//!
//! Declarative, no I/O -- the same rule `domain` follows for the billing
//! module. This crate names no crate from the billing module either, and no
//! billing crate names it: the host composes the two, and neither module
//! learns the other exists. Both checks run in CI.
//!
//! # Secrets are types, not conventions
//!
//! A password never travels as a `String`. [`Password`] wraps a
//! `SecretString`: its `Debug` is redacted, and it has no `Display` and no
//! `Serialize`, so it cannot be formatted into a log line or written into a
//! response body by accident. [`NewPassword`] is the only thing a hasher
//! accepts, and the only way to get one is through the policy check -- a
//! password that was never checked cannot be stored.

mod email;
mod password;
mod role;
mod token;

pub use email::{Email, EmailError};
pub use password::{
    MAX_PASSWORD_CHARS, MIN_PASSWORD_CHARS, NewPassword, Password, PasswordPolicyError,
};
pub use role::{Role, RoleParseError};
pub use token::{
    SELECTOR_BYTES, Selector, SplitToken, TokenParseError, VERIFIER_BYTES, Verifier, VerifierHash,
};
