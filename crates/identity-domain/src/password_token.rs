use std::fmt;

use audit::CorrelationId;
use secrecy::SecretString;
use time::OffsetDateTime;

use crate::{Email, Selector, UserId, VerifierHash};

/// What a password token lets its holder do. Both set a password; they
/// differ in how long they live and in the words of the mail that carries
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenPurpose {
    /// "I forgot my password."
    PasswordReset,
    /// "You were added to a tenant; choose a password."
    Invitation,
}

impl TokenPurpose {
    /// The stored spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            TokenPurpose::PasswordReset => "password_reset",
            TokenPurpose::Invitation => "invitation",
        }
    }
}

/// A reset or invitation token to be stored: selector and verifier hash,
/// never the verifier -- the same shape, for the same reason, as
/// [`NewSession`](crate::NewSession). A copy of the tokens table cannot be
/// turned back into a working link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPasswordToken {
    /// Finds the row.
    pub selector: Selector,
    /// `SHA-256(verifier)`.
    pub verifier_hash: VerifierHash,
    /// Whose password the token sets.
    pub user_id: UserId,
    /// Reset or invitation.
    pub purpose: TokenPurpose,
    /// When it was issued, by the module's clock.
    pub created_at: OffsetDateTime,
    /// When it stops working, by the same clock.
    pub expires_at: OffsetDateTime,
}

/// An outstanding token as stored, found by its selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPasswordToken {
    /// Whose password it sets.
    pub user_id: UserId,
    /// Reset or invitation.
    pub purpose: TokenPurpose,
    /// `SHA-256(verifier)`, to check a presented verifier against.
    pub verifier_hash: VerifierHash,
    /// When it stops working.
    pub expires_at: OffsetDateTime,
}

/// A mail the module sends. The link is the whole credential, so it is a
/// secret here and `Debug` does not print it; only a [`Mailer`] ever
/// exposes it, into the message body.
///
/// [`Mailer`]: crate::Mailer
pub struct OutgoingMail {
    /// The recipient.
    pub to: Email,
    /// Which message.
    pub purpose: TokenPurpose,
    /// The link carrying the token.
    pub link: SecretString,
    /// The request that caused the mail, so a mail log line joins its audit
    /// rows.
    pub correlation_id: CorrelationId,
}

impl fmt::Debug for OutgoingMail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutgoingMail")
            .field("to", &self.to)
            .field("purpose", &self.purpose)
            .field("link", &"[REDACTED]")
            .field("correlation_id", &self.correlation_id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn debug_never_prints_the_link() -> Result<(), crate::EmailError> {
        let mail = OutgoingMail {
            to: Email::parse("alice@example.test")?,
            purpose: TokenPurpose::PasswordReset,
            link: SecretString::from("http://localhost/reset-password#token=abc.def"),
            correlation_id: CorrelationId::new(Uuid::new_v4()),
        };
        let rendered = format!("{mail:?}");
        assert!(!rendered.contains("abc.def"));
        assert!(!rendered.contains("alice"));
        Ok(())
    }
}
