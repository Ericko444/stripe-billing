//! `demo`'s own JWT extractor. `api` defines the extractor contract
//! ([`TenantExtractor`](api::TenantExtractor)) and has no use for a JWT
//! itself (D1) -- authentication is the host's job.
//!
//! **The pin is the whole defence.** `Validation::new(Algorithm::HS256)`
//! below fixes the only algorithm this decoder ever accepts. An unpinned
//! `Validation` accepts whatever algorithm a token's own header names --
//! including `alg: none` -- so an attacker who can produce *any* token can
//! re-sign it with a family the verifier still accepts and walk straight
//! past the signature check.

use api::{ApiError, AppState};
use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use domain::TenantId;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The claims a `demo` token carries. `sub` is the tenant's uuid directly
/// (Open Question 3 -- `sub`, not a custom `tenant_id` claim); `exp` is the
/// standard expiry `jsonwebtoken` checks during decode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// The tenant this token authenticates as.
    pub sub: Uuid,
    /// Unix timestamp the token expires at.
    pub exp: usize,
}

/// Verifies `demo`'s own tokens against `BILLING_JWT_SECRET`. Travels in a
/// request `Extension` (D3) rather than `AppState`: `api` never sees it, and
/// `demo`, as the composition root, is the one place that mechanism is
/// acceptable.
#[derive(Clone)]
pub struct JwtDecoder {
    decoding_key: DecodingKey,
    validation: Validation,
}

impl JwtDecoder {
    /// Builds a decoder pinned to HS256 (see module docs) over `secret`.
    pub fn new(secret: &SecretString) -> Self {
        Self {
            decoding_key: DecodingKey::from_secret(secret.expose_secret().as_bytes()),
            validation: Validation::new(Algorithm::HS256),
        }
    }

    /// Verifies and decodes `token`, checking signature, algorithm and
    /// expiry in one call. Any failure -- including a well-formed-but-wrong
    /// `sub` -- is reported as a single opaque error; the caller must not
    /// distinguish causes from it (D5).
    fn decode(&self, token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
        jsonwebtoken::decode::<Claims>(token, &self.decoding_key, &self.validation)
            .map(|data| data.claims)
    }
}

/// The tenant a valid `demo` token names. Satisfies `TenantExtractor`'s bound
/// by blanket impl: an Axum extractor rejecting with [`ApiError`], and
/// [`Into<TenantId>`].
#[derive(Debug, Clone, Copy)]
pub struct DemoTenant(TenantId);

impl From<DemoTenant> for TenantId {
    fn from(tenant: DemoTenant) -> Self {
        tenant.0
    }
}

impl FromRequestParts<AppState> for DemoTenant {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Every rejection below is `ApiError::Unauthorized` and nothing
        // else: no header, no `Bearer ` prefix, wrong secret, wrong
        // algorithm, an expired token, a non-uuid `sub`, and a missing
        // decoder all produce the identical body (D5) -- the response alone
        // must never tell a caller which of these happened.
        let decoder = parts
            .extensions
            .get::<JwtDecoder>()
            .ok_or(ApiError::Unauthorized)?;

        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(ApiError::Unauthorized)?;

        let claims = decoder.decode(token).map_err(|_| ApiError::Unauthorized)?;

        Ok(DemoTenant(TenantId::new(claims.sub)))
    }
}
