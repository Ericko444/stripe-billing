use secrecy::ExposeSecret;

use crate::{StripeConfig, StripeError};

/// Builds a Stripe `Client` from a `StripeConfig`.
///
/// Applies `base_url` as a `ClientBuilder::url` override when set. The
/// SDK's own doc comment on `url` reads "useful for testing," which is
/// exactly and only how this crate uses it: pointing the client at a
/// `wiremock` server instead of Stripe's real host (`api.stripe.com`, the
/// SDK's built-in default, used whenever `base_url` is `None`).
pub fn build_client(config: &StripeConfig) -> Result<stripe::Client, StripeError> {
    let mut builder = stripe::ClientBuilder::new(config.secret.expose_secret());
    if let Some(base_url) = &config.base_url {
        builder = builder.url(base_url.clone());
    }
    builder.build().map_err(StripeError::from)
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;
    use stripe_client_core::{RequestBuilder, StripeMethod};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[tokio::test]
    async fn base_url_override_routes_a_request_to_the_mock()
    -> Result<(), Box<dyn std::error::Error>> {
        let mock_server = MockServer::start().await;
        // The client joins api_base + "v1" + the request path
        // (async-stripe/src/hyper/client.rs), so a request built with path
        // "/ping" lands at "/v1/ping".
        Mock::given(method("GET"))
            .and(path("/v1/ping"))
            .respond_with(ResponseTemplate::new(200).set_body_json("ok"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = StripeConfig {
            secret: SecretString::from("sk_test_fake".to_string()),
            base_url: Some(mock_server.uri()),
        };
        let client = build_client(&config)?;

        let response: String = RequestBuilder::new(StripeMethod::Get, "/ping")
            .customize()
            .send(&client)
            .await?;

        assert_eq!(response, "ok");
        // Confirms the mounted expectation (exactly one matching request)
        // was actually satisfied, not just that *some* request succeeded.
        mock_server.verify().await;
        Ok(())
    }

    #[test]
    fn no_base_url_targets_stripes_real_host() {
        let config = StripeConfig {
            secret: SecretString::from("sk_test_fake".to_string()),
            base_url: None,
        };

        // build_client succeeding with no base_url is the only thing
        // observable from outside the SDK without a real network call --
        // the SDK's own default (api.stripe.com) is exercised by the SDK's
        // own tests, not ours. What matters here is that omitting base_url
        // does not accidentally require one.
        assert!(build_client(&config).is_ok());
    }
}
