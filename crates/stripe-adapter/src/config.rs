use core::fmt;

use secrecy::SecretString;
use time::Duration;

/// Configuration needed to construct a Stripe client.
///
/// `secret` is wrapped in `secrecy::SecretString`, which already redacts on
/// `Debug`. The manual `Debug` impl below is belt-and-braces on top of that,
/// not a substitute for it: the failure mode it guards against is someone
/// later adding a plain, unwrapped `String` field next to `secret` and
/// getting a `#[derive(Debug)]` that prints it. A manual impl forces every
/// new field to be a deliberate addition here, not an automatic one. The
/// leak it prevents is a `Debug`-printed config struct feeding a tracing
/// span.
pub struct StripeConfig {
    /// The Stripe secret API key.
    pub secret: SecretString,
    /// Overrides the client's base URL. `None` in production; `Some` in
    /// tests, to point the client at a `wiremock` server instead of
    /// Stripe's real host (`ClientBuilder::url`, used in `client.rs`).
    pub base_url: Option<String>,
}

impl fmt::Debug for StripeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StripeConfig")
            .field("secret", &"[redacted]")
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// The exact Stripe API version this adapter is pinned against, read from
/// the compiled SDK rather than duplicated as a string literal here -- so a
/// version bump in `Cargo.toml` is what changes this, not a second place
/// that could silently drift from the first. Exposed so a host can assert
/// at startup that it matches the API version its Stripe account and
/// webhook endpoint are configured for.
///
/// A plain function rather than a `const`: `ApiVersion::as_str` is not a
/// `const fn`, and duplicating its match arms here just to get a `const`
/// would reintroduce exactly the two-places-that-can-drift problem this
/// exists to avoid.
pub fn pinned_api_version() -> &'static str {
    stripe_client_core::VERSION.as_str()
}

/// Configuration the webhook verifier needs: the endpoint's signing secret
/// and the replay-window tolerance.
///
/// Deliberately separate from [`StripeConfig`]. The verifier needs the
/// signing secret and has no use for the API key or the base URL; bundling
/// them would mean the verifier holds a credential it never uses -- the kind
/// of thing that becomes an accidental log line later.
///
/// `signing_secret` is a `secrecy::SecretString`, which already redacts on
/// `Debug`. The manual `Debug` impl below is the same belt-and-braces as
/// `StripeConfig`'s: it guards against a future plain, unwrapped `String`
/// field getting a `#[derive(Debug)]` that prints it.
///
/// `tolerance` is a field rather than a constant because hand-rolling the
/// verifier made the replay window configurable; [`DEFAULT_TOLERANCE`] is
/// the value to use when there is no reason to deviate.
///
/// [`DEFAULT_TOLERANCE`]: crate::DEFAULT_TOLERANCE
pub struct WebhookConfig {
    /// The endpoint's Stripe webhook signing secret (`whsec_...`).
    pub signing_secret: SecretString,
    /// How far the signature header's timestamp may be from now before a
    /// delivery is rejected as a replay. Use `crate::DEFAULT_TOLERANCE`
    /// unless a specific deployment needs otherwise.
    pub tolerance: Duration,
}

impl fmt::Debug for WebhookConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebhookConfig")
            .field("signing_secret", &"[redacted]")
            .field("tolerance", &self.tolerance)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_does_not_contain_the_secret() {
        let config = StripeConfig {
            secret: SecretString::from("sk_test_should_never_appear_in_debug_output".to_string()),
            base_url: Some("http://127.0.0.1:1".to_string()),
        };

        let rendered = format!("{config:?}");

        assert!(!rendered.contains("sk_test_should_never_appear_in_debug_output"));
    }

    #[test]
    fn pinned_api_version_matches_the_locked_rc() {
        // A version bump in Cargo.toml (Cargo.lock, really) should make this
        // fail loudly rather than let the pin silently drift.
        assert_eq!(pinned_api_version(), "2026-07-29.dahlia");
    }

    #[test]
    fn webhook_debug_does_not_contain_the_signing_secret() {
        let config = WebhookConfig {
            signing_secret: SecretString::from(
                "whsec_should_never_appear_in_debug_output".to_string(),
            ),
            tolerance: crate::DEFAULT_TOLERANCE,
        };

        let rendered = format!("{config:?}");

        assert!(!rendered.contains("whsec_should_never_appear_in_debug_output"));
    }
}
