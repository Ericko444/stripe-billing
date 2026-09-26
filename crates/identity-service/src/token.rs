use identity_domain::{SELECTOR_BYTES, SplitToken, VERIFIER_BYTES};
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

/// A fresh [`SplitToken`] from the operating system's CSPRNG.
///
/// `getrandom` directly, not a user-space PRNG seeded from it: there is no
/// throughput to win here, and every layer between the OS and the verifier
/// is a place a seeding or reuse bug could hide.
pub fn generate_token() -> Result<SplitToken, TokenGenerationError> {
    let mut selector = [0u8; SELECTOR_BYTES];
    let mut verifier = [0u8; VERIFIER_BYTES];
    getrandom::fill(&mut selector).map_err(|err| TokenGenerationError(err.to_string()))?;
    getrandom::fill(&mut verifier).map_err(|err| TokenGenerationError(err.to_string()))?;
    Ok(SplitToken::from_bytes(selector, verifier))
}

/// `{base}/reset-password#token={token}` -- the one page that sets a
/// password from a link, for resets and invitations alike.
///
/// The token rides in the **fragment**. A fragment is never sent to a
/// server, so the credential stays out of access logs, proxy logs and the
/// `Referer` of anything the page loads; the page reads it with script and
/// sends it in a request body.
pub(crate) fn password_link(base: &str, token: &SplitToken) -> SecretString {
    SecretString::from(format!(
        "{}/reset-password#token={}",
        base.trim_end_matches('/'),
        token.to_wire().expose_secret()
    ))
}

/// The OS random source failed. Not recoverable by retrying with less
/// randomness -- the caller fails the request.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("token generation failed: {0}")]
pub struct TokenGenerationError(pub String);

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    #[test]
    fn two_tokens_differ_in_both_halves() -> Result<(), TokenGenerationError> {
        let first = generate_token()?;
        let second = generate_token()?;

        assert_ne!(first.selector(), second.selector());
        assert!(!first.verifier().hash().verifies(second.verifier()));
        assert_ne!(
            first.to_wire().expose_secret(),
            second.to_wire().expose_secret()
        );
        Ok(())
    }
}
