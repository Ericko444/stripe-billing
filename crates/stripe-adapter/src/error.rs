use domain::DomainError;
use thiserror::Error;

/// Errors from the Stripe adapter: transport failures, response
/// deserialization failures, API errors Stripe itself returned, and client
/// configuration errors.
///
/// Deliberately distinct from the SDK's own `stripe::StripeError` (this
/// crate is named `stripe-adapter`, not `stripe`, precisely so `stripe::`
/// unambiguously means the SDK, whose lib name *is* `stripe`). Everything
/// in this crate and its tests is written against `StripeError`, so a
/// future SDK version reshaping its own error type only requires updating
/// the `From` impl below, not every call site.
#[derive(Debug, Error)]
pub enum StripeError {
    /// The request could not reach Stripe, or the transport itself failed
    /// (DNS, TLS, connection reset).
    #[error("transport error: {0}")]
    Transport(String),

    /// Stripe's response body could not be parsed into the expected shape.
    #[error("deserialization error: {0}")]
    Deserialization(String),

    /// Stripe accepted the request but returned an API-level error (a 4xx
    /// or 5xx with a Stripe error body).
    #[error("stripe API error: {message}")]
    Api {
        /// Stripe's own error message, if the response body had one.
        message: String,
        /// The HTTP status code Stripe responded with.
        status: u16,
        /// A link to this exact request in the Stripe dashboard's request
        /// log, if Stripe included one.
        ///
        /// Not a bare `request_id` string: verified against the crate
        /// (`stripe_shared::ApiErrors`, `=1.0.0-rc.8`) that no such field
        /// exists anywhere the SDK exposes to a caller -- neither on the
        /// deserialized error body nor as a response header the client
        /// abstraction surfaces (`StripeClient::execute` returns only
        /// `Result<Bytes, Self::Err>`, headers discarded). This URL is the
        /// closest equivalent Stripe actually returns, and it arguably
        /// serves production traceability better than a bare id would: it's
        /// directly clickable, not something that needs a separate lookup.
        request_log_url: Option<String>,
    },

    /// `StripeConfig` could not be turned into a usable client.
    #[error("configuration error: {0}")]
    Config(String),
}

impl From<stripe::StripeError> for StripeError {
    fn from(err: stripe::StripeError) -> Self {
        match err {
            stripe::StripeError::Stripe(api_errors, status) => StripeError::Api {
                message: api_errors
                    .message
                    .clone()
                    .unwrap_or_else(|| "Stripe returned no error message".to_string()),
                status,
                request_log_url: api_errors.request_log_url.clone(),
            },
            stripe::StripeError::JSONDeserialize(msg) => StripeError::Deserialization(msg),
            stripe::StripeError::ClientError(msg) => StripeError::Transport(msg),
            stripe::StripeError::ConfigError(msg) => StripeError::Config(msg),
            stripe::StripeError::Timeout => {
                StripeError::Transport("timeout communicating with stripe".to_string())
            }
        }
    }
}

/// Flattens a `StripeError` into `DomainError::Provider`. The rich type
/// (`status`, `request_log_url`) stays inside `stripe-adapter` for logs;
/// `domain` only ever sees the coarse string `StripeError`'s own `Display`
/// produces (`domain` must not depend on this crate's error type any more
/// than it depends on `stripe` itself).
///
/// Deliberately coarse, and deliberately routed through `Display` rather
/// than `Debug`: each variant's `#[error("...")]` format string names
/// exactly which fields are safe to surface, so a field added to `Api` later
/// (or to any variant) does not automatically leak into the domain-facing
/// message the way a `{:?}` dump would -- the mapping is where leaks happen.
impl From<StripeError> for DomainError {
    fn from(err: StripeError) -> Self {
        DomainError::Provider(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_flattens_to_only_the_message_field() {
        // request_log_url is real, non-secret data Stripe sends -- but the
        // point of this test isn't that this particular field is sensitive.
        // It stands in for "any field beyond the intended one," proving the
        // Display-based flattening surfaces exactly what its format string
        // names and nothing else. If a future edit turned `Api`'s
        // `#[error("stripe API error: {message}")]` into something that
        // formats the whole variant (e.g. `{self:?}`), this test would catch
        // it -- and that same mistake is exactly how a real secret or a raw
        // response body would leak into a domain-facing error.
        let err = StripeError::Api {
            message: "Your card was declined.".to_string(),
            status: 402,
            request_log_url: Some(
                "https://dashboard.stripe.com/logs/req_should_not_leak".to_string(),
            ),
        };

        let domain_err: DomainError = err.into();
        let rendered = domain_err.to_string();

        assert!(rendered.contains("Your card was declined."));
        assert!(
            !rendered.contains("req_should_not_leak"),
            "the domain-facing message must not leak fields beyond the intended one: {rendered}"
        );
        assert!(
            !rendered.contains("402"),
            "the domain-facing message must not leak the status code: {rendered}"
        );
    }
}
