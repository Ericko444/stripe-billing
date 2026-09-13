use std::fmt;

use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

/// Shortest password accepted, in Unicode scalar values.
///
/// NIST SP 800-63B rev. 4 sets 15 as the minimum when a password is the
/// only authentication factor, which it is here. No composition rules
/// (digits, symbols, mixed case) and no expiry, per the same guidance:
/// both push users toward predictable patterns rather than stronger ones.
pub const MIN_PASSWORD_CHARS: usize = 15;

/// Longest password accepted, in Unicode scalar values.
///
/// NIST asks for at least 64. The cap exists to bound request size, not
/// hashing cost -- Argon2id's cost is dominated by its memory parameter, not
/// by input length.
pub const MAX_PASSWORD_CHARS: usize = 128;

/// A password as presented by a caller, unchecked.
///
/// This is what login verifies against a stored hash, and it is
/// deliberately *not* checked against the policy: a password set under an
/// older policy must still verify, and refusing it with a policy message
/// would tell a caller something about the account.
///
/// It cannot be printed or serialised. `Debug` is redacted, and there is no
/// `Display` or `Serialize`:
///
/// ```compile_fail
/// use identity_domain::Password;
///
/// let password = Password::new("correct horse battery staple".to_string());
/// // Does not compile: `Password` has no `Display`.
/// let _ = format!("{password}");
/// ```
///
/// ```compile_fail
/// use identity_domain::Password;
///
/// fn requires_serialize<T: serde::Serialize>(_: &T) {}
///
/// let password = Password::new("correct horse battery staple".to_string());
/// // Does not compile: `Password` is not `Serialize`.
/// requires_serialize(&password);
/// ```
///
/// The two examples above fail for the reason their comments give, and not
/// for a typo: the same imports and constructor compile here.
///
/// ```
/// use identity_domain::Password;
///
/// fn requires_debug<T: std::fmt::Debug>(_: &T) {}
///
/// let password = Password::new("correct horse battery staple".to_string());
/// requires_debug(&password);
/// assert!(!format!("{password:?}").contains("horse"));
/// ```
pub struct Password(SecretString);

impl Password {
    /// Wraps `raw`. The `String` is moved into the secret, so no plain copy
    /// is left behind by this call.
    pub fn new(raw: String) -> Self {
        Self(SecretString::from(raw))
    }

    /// The password's characters, for a hasher to consume. Named after
    /// `secrecy`'s own method so every read of a secret is greppable.
    pub fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password([REDACTED])")
    }
}

/// A password that has passed the policy -- the only kind that may be
/// hashed and stored.
///
/// There is no public constructor other than [`NewPassword::check`], so "a
/// password was stored without being checked" is a type error rather than a
/// review finding.
pub struct NewPassword(Password);

impl NewPassword {
    /// Checks `password` against the length policy.
    pub fn check(password: Password) -> Result<Self, PasswordPolicyError> {
        let chars = password.expose_secret().chars().count();
        if chars < MIN_PASSWORD_CHARS {
            return Err(PasswordPolicyError::TooShort);
        }
        if chars > MAX_PASSWORD_CHARS {
            return Err(PasswordPolicyError::TooLong);
        }
        Ok(Self(password))
    }

    /// The checked password.
    pub fn as_password(&self) -> &Password {
        &self.0
    }
}

impl fmt::Debug for NewPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NewPassword([REDACTED])")
    }
}

/// A stored password hash: an Argon2id PHC string
/// (`$argon2id$v=19$m=…,t=…,p=…$salt$hash`), which records its own
/// algorithm, parameters and salt.
///
/// Not a secret in the way a password is, but it is offline-cracking
/// material, so `Debug` is redacted all the same.
#[derive(Clone, PartialEq, Eq)]
pub struct PasswordHash(String);

impl PasswordHash {
    /// Wraps a PHC string, as produced by a hasher or read from storage.
    pub fn new(phc: String) -> Self {
        Self(phc)
    }

    /// The PHC string, for storage or for a hasher to parse.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PasswordHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PasswordHash(..)")
    }
}

/// Why a new password was refused. Safe to show a caller: it describes the
/// rule, not the password, and it is only ever returned for a password the
/// caller is *setting*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PasswordPolicyError {
    /// Fewer than [`MIN_PASSWORD_CHARS`] characters.
    #[error("password is shorter than {MIN_PASSWORD_CHARS} characters")]
    TooShort,
    /// More than [`MAX_PASSWORD_CHARS`] characters.
    #[error("password is longer than {MAX_PASSWORD_CHARS} characters")]
    TooLong,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of_len(chars: usize) -> Password {
        Password::new("x".repeat(chars))
    }

    #[test]
    fn policy_bounds_are_inclusive() {
        assert_eq!(
            NewPassword::check(of_len(MIN_PASSWORD_CHARS - 1)).err(),
            Some(PasswordPolicyError::TooShort)
        );
        assert!(NewPassword::check(of_len(MIN_PASSWORD_CHARS)).is_ok());
        assert!(NewPassword::check(of_len(MAX_PASSWORD_CHARS)).is_ok());
        assert_eq!(
            NewPassword::check(of_len(MAX_PASSWORD_CHARS + 1)).err(),
            Some(PasswordPolicyError::TooLong)
        );
    }

    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        // 15 two-byte characters: 30 bytes, 15 characters -- accepted.
        let password = Password::new("é".repeat(MIN_PASSWORD_CHARS));
        assert!(NewPassword::check(password).is_ok());
    }

    #[test]
    fn debug_is_redacted_for_both_types() {
        let secret = "correct horse battery staple";
        let password = Password::new(secret.to_string());
        assert!(!format!("{password:?}").contains("horse"));

        let checked = NewPassword::check(Password::new(secret.to_string()));
        assert!(!format!("{checked:?}").contains("horse"));
    }
}
