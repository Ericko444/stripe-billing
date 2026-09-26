use std::fmt;

use sha2::{Digest, Sha256};
use thiserror::Error;

/// Longest address accepted, in bytes: the path limit RFC 5321 places on a
/// forward path, less the angle brackets.
const MAX_EMAIL_BYTES: usize = 254;

/// A normalised email address -- the key a user is found by.
///
/// Normalisation is trim, then ASCII-lowercase of the **whole** address.
/// RFC 5321 lets a local part be case-sensitive, but no mainstream provider
/// treats it so, and two accounts differing only by case are a support
/// incident and a phishing vector. One address, one account.
///
/// Validation is deliberately shallow: exactly one `@`, a non-empty local
/// part and domain, no whitespace or control characters, at most 254 bytes.
/// Whether an address can receive mail is decided by sending to it, not by
/// a regular expression. Internationalised addresses are not normalised
/// beyond ASCII case -- a named limit.
///
/// `Debug` is redacted. An address is personal data, and this type is the
/// one most likely to be dropped into a `tracing` field "for context"; a
/// log that must be erasable should hold a hash of it, not the address.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Email(String);

impl Email {
    /// Parses and normalises `raw`.
    pub fn parse(raw: &str) -> Result<Self, EmailError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.len() > MAX_EMAIL_BYTES {
            return Err(EmailError);
        }
        if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(EmailError);
        }
        let Some((local, domain)) = trimmed.split_once('@') else {
            return Err(EmailError);
        };
        if local.is_empty() || domain.is_empty() || domain.contains('@') {
            return Err(EmailError);
        }
        Ok(Self(trimmed.to_ascii_lowercase()))
    }

    /// The normalised address.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `hex(SHA-256(address))` -- a stable, non-reversible handle on an
    /// address, for a log line or a rate-limit key that must not hold the
    /// address itself. Two spellings of one address share a fingerprint.
    pub fn fingerprint(&self) -> String {
        hex::encode(Sha256::digest(self.0.as_bytes()))
    }
}

impl fmt::Debug for Email {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Email([REDACTED])")
    }
}

/// An address did not parse. Carries no detail: which rule failed is of no
/// use to a caller, and echoing the input back is echoing personal data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("invalid email address")]
pub struct EmailError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_lowercases() {
        assert_eq!(
            Email::parse("  Alice@Example.TEST ").map(|e| e.as_str().to_string()),
            Ok("alice@example.test".to_string())
        );
    }

    #[test]
    fn two_spellings_of_one_address_are_equal() {
        assert_eq!(
            Email::parse("BOB@example.test"),
            Email::parse("bob@EXAMPLE.test")
        );
    }

    #[test]
    fn rejects_malformed_addresses() {
        for raw in [
            "",
            "   ",
            "no-at-sign",
            "@example.test",
            "alice@",
            "a@b@c",
            "al ice@example.test",
            "alice@exam\tple.test",
        ] {
            assert_eq!(Email::parse(raw), Err(EmailError), "{raw:?}");
        }
    }

    #[test]
    fn enforces_the_length_limit() {
        let domain = "@example.test";
        let at_limit = format!("{}{domain}", "a".repeat(MAX_EMAIL_BYTES - domain.len()));
        let over = format!("a{at_limit}");

        assert!(Email::parse(&at_limit).is_ok());
        assert_eq!(Email::parse(&over), Err(EmailError));
    }

    #[test]
    fn debug_does_not_print_the_address() {
        let rendered = format!("{:?}", Email::parse("alice@example.test"));
        assert!(!rendered.contains("alice"));
        assert!(!rendered.contains("example"));
    }
}
