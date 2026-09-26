use domain::DomainError;
use thiserror::Error;

/// Why a webhook delivery failed verification, at the granularity this crate
/// logs server-side.
///
/// The four variants exist for the operator reading logs with a correlation
/// id, not for the caller: [`From<WebhookError> for DomainError`] collapses
/// **all four** to [`DomainError::WebhookVerification`], which carries no
/// detail (see that impl). `StripeError` flattens to a `String` to protect
/// a crate boundary (`domain` stays free of adapter types); this flattens
/// to *nothing* to protect
/// an untrusted caller. Both are boundary decisions; only this one is a
/// security control.
#[derive(Debug, Error)]
pub enum WebhookError {
    /// No `v1` signature in the header matched the HMAC of the signed
    /// payload. Covers a forged signature, a tampered body, and the wrong
    /// signing secret alike — none are distinguished, deliberately.
    #[error("no matching v1 signature")]
    Signature,

    /// The header timestamp is outside the tolerated replay window. The
    /// detail is the observed skew, for logs only.
    #[error("timestamp outside tolerance: {0}")]
    Timestamp(String),

    /// The `Stripe-Signature` header could not be parsed: a missing or
    /// unparseable `t`, or no `v1` value at all.
    #[error("malformed signature header: {0}")]
    MalformedHeader(String),

    /// The body passed verification but was not valid UTF-8, or not the
    /// JSON event envelope the ledger needs.
    #[error("unparseable payload: {0}")]
    Payload(String),
}

/// Collapses every [`WebhookError`] to the single opaque
/// [`DomainError::WebhookVerification`].
///
/// Intentionally many-to-one, which is the opposite of what error-handling
/// instinct suggests. *Why* verification failed — bad signature vs. expired
/// timestamp vs. malformed header — is information an attacker probing a
/// public, unauthenticated endpoint can use. The rich `WebhookError` stays
/// inside `stripe-adapter` and is logged with a correlation id; `domain`
/// and everything above it see only "webhook verification failed" -- the
/// mapping is where leaks happen.
impl From<WebhookError> for DomainError {
    fn from(_err: WebhookError) -> Self {
        DomainError::WebhookVerification
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant — including the ones carrying a detail string — maps to
    /// the same payload-free domain variant.
    #[test]
    fn all_variants_map_to_webhook_verification() {
        let cases = [
            WebhookError::Signature,
            WebhookError::Timestamp("skewed by 4000s".to_string()),
            WebhookError::MalformedHeader("missing t".to_string()),
            WebhookError::Payload("expected value at line 1 column 1".to_string()),
        ];

        for case in cases {
            assert_eq!(DomainError::from(case), DomainError::WebhookVerification);
        }
    }

    /// The no-oracle rule, pinned by a test rather than by intent: the
    /// domain-facing `Display` is byte-for-byte identical across variants and
    /// leaks none of the detail the `WebhookError` carried.
    #[test]
    fn domain_display_reveals_nothing_about_which_check_failed() {
        let cases = [
            (WebhookError::Signature, "signature"),
            (
                WebhookError::Timestamp("skewed by 4000s".to_string()),
                "4000s",
            ),
            (
                WebhookError::MalformedHeader("missing t".to_string()),
                "missing t",
            ),
            (
                WebhookError::Payload("expected value at line 1 column 1".to_string()),
                "line 1",
            ),
        ];

        for (err, leak_fragment) in cases {
            let rendered = DomainError::from(err).to_string();
            assert_eq!(
                rendered, "webhook verification failed",
                "the domain-facing message must be the same for every failure reason"
            );
            assert!(
                !rendered.contains(leak_fragment),
                "the domain-facing message leaked a detail: {rendered}"
            );
        }
    }
}
