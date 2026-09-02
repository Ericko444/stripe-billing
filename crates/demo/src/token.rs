//! `POST /demo/token`: scaffolding that mints `demo`'s own tokens.
//!
//! **Not a real host's login route.** It takes a tenant id and hands back a
//! token for it -- no password, no session, no identity check of any kind.
//! A real host authenticates a caller first and only then decides which
//! tenant they are; this route exists so a reviewer without one can still
//! drive the tenant-scoped surface, and `main` logs a warning naming it at
//! every startup so that scaffolding is never mistaken for the real thing.

use api::ApiError;
use axum::Router;
use axum::extract::State;
use axum::routing::post;
use domain::DomainError;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime, UtcOffset};
use uuid::Uuid;

use crate::jwt::Claims;

/// Signs `demo`'s own tokens with the same `BILLING_JWT_SECRET` [`JwtDecoder`](crate::jwt::JwtDecoder)
/// verifies them against.
#[derive(Clone)]
struct TokenIssuer {
    encoding_key: EncodingKey,
}

impl TokenIssuer {
    fn new(secret: &SecretString) -> Self {
        Self {
            encoding_key: EncodingKey::from_secret(secret.expose_secret().as_bytes()),
        }
    }

    /// Mints a token for `tenant`, expiring one hour from now. The only
    /// realistic failure is a `jsonwebtoken` internal error -- `Claims` is a
    /// plain `Uuid` + `usize` pair, so encoding it cannot fail in practice;
    /// callers still see it as an opaque 500 rather than this crate
    /// asserting an invariant it cannot fully prove.
    fn mint(&self, tenant: Uuid) -> Result<(String, OffsetDateTime), jsonwebtoken::errors::Error> {
        let expires_at = OffsetDateTime::now_utc() + Duration::hours(1);
        let claims = Claims {
            sub: tenant,
            exp: expires_at.unix_timestamp() as usize,
        };
        let token = encode(&Header::new(Algorithm::HS256), &claims, &self.encoding_key)?;
        Ok((token, expires_at))
    }
}

#[derive(Debug, Deserialize)]
struct MintTokenRequest {
    tenant_id: String,
}

#[derive(Debug, Serialize)]
struct MintTokenResponse {
    token: String,
    expires_at: String,
}

/// Formats an [`OffsetDateTime`] as an RFC 3339 UTC string with second
/// precision. Built from the datetime's own fields, matching `api`'s own
/// `rfc3339_utc`, rather than pulling in `time`'s `formatting` feature for
/// one call site.
fn rfc3339_utc(datetime: OffsetDateTime) -> String {
    let datetime = datetime.to_offset(UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        datetime.year(),
        u8::from(datetime.month()),
        datetime.day(),
        datetime.hour(),
        datetime.minute(),
        datetime.second(),
    )
}

async fn mint_token(
    State(issuer): State<TokenIssuer>,
    axum::Json(request): axum::Json<MintTokenRequest>,
) -> Result<axum::Json<MintTokenResponse>, ApiError> {
    // A malformed uuid is the caller's mistake -- 400, not the 500 an
    // unhandled parse failure would give it (D5's "never leak the cause"
    // logic doesn't apply here: this route has no auth to protect).
    let tenant_id = Uuid::parse_str(&request.tenant_id).map_err(|_| {
        ApiError::from(DomainError::MalformedRequest(
            "tenant_id is not a valid uuid".to_string(),
        ))
    })?;

    let (token, expires_at) = issuer
        .mint(tenant_id)
        .map_err(|err| ApiError::from(DomainError::Repository(format!("token encoding: {err}"))))?;

    Ok(axum::Json(MintTokenResponse {
        token,
        expires_at: rfc3339_utc(expires_at),
    }))
}

/// Builds the `/demo/token` route as its own state-erased `Router`, ready to
/// `.merge()` alongside `billing_router` and `webhook_router`.
pub fn demo_token_router(secret: &SecretString) -> Router {
    Router::new()
        .route("/demo/token", post(mint_token))
        .with_state(TokenIssuer::new(secret))
}
