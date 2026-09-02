//! `DemoTenant`'s full rejection-path suite (Plan 4d Task 4, P2): built and
//! tested while `demo` still serves only the webhook route, so nothing here
//! depends on a mount that does not exist yet.
//!
//! "The token was refused" is worthless if it was refused for the wrong
//! reason, so every rejection path gets its own named test -- and the row
//! that matters most, [`jwt_every_rejection_looks_the_same`], checks that all
//! seven produce the byte-identical `detail` (D5): the response alone must
//! never tell a caller *why* it was rejected.

mod common;

use std::error::Error;

use api::ApiError;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::Extension;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use common::unused_state;
use demo::jwt::{Claims, DemoTenant, JwtDecoder};
use jsonwebtoken::{Algorithm, EncodingKey, Header as JwtHeader, encode};
use secrecy::SecretString;
use serde::Serialize;
use serde_json::Value;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "the-demo-signing-secret";

async fn whoami(tenant: DemoTenant) -> impl IntoResponse {
    let tenant_id: domain::TenantId = tenant.into();
    tenant_id.as_uuid().to_string()
}

/// A router with exactly one route behind `DemoTenant`, mirroring how
/// `main` will mount `billing_router::<DemoTenant>` (Task 5) -- except this
/// exercises the extractor alone, before anything else depends on it.
/// `decoder: None` stands in for the layer never having been installed.
fn router(decoder: Option<JwtDecoder>) -> Router {
    let router = Router::new()
        .route("/whoami", get(whoami))
        .with_state(unused_state());
    match decoder {
        Some(decoder) => router.layer(Extension(decoder)),
        None => router,
    }
}

fn valid_claims(sub: Uuid) -> Claims {
    Claims {
        sub,
        exp: (OffsetDateTime::now_utc() + Duration::hours(1)).unix_timestamp() as usize,
    }
}

fn token_with<T: Serialize>(
    claims: &T,
    secret: &str,
    alg: Algorithm,
) -> Result<String, Box<dyn Error>> {
    let key = EncodingKey::from_secret(secret.as_bytes());
    Ok(encode(&JwtHeader::new(alg), claims, &key)?)
}

fn decoder() -> JwtDecoder {
    JwtDecoder::new(&SecretString::from(SECRET.to_string()))
}

/// Drives `app` with an optional `Authorization` header and returns the
/// status plus the parsed `problem+json` body.
async fn call(
    app: Router,
    auth_header: Option<&str>,
) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let mut request = Request::builder().uri("/whoami");
    if let Some(value) = auth_header {
        request = request.header(header::AUTHORIZATION, value);
    }
    let response = app.oneshot(request.body(Body::empty())?).await?;

    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let body = serde_json::from_slice(&bytes)?;
    Ok((status, body))
}

#[tokio::test]
async fn jwt_valid_token_reaches_the_handler_with_its_tenant() -> Result<(), Box<dyn Error>> {
    let tenant = Uuid::new_v4();
    let token = token_with(&valid_claims(tenant), SECRET, Algorithm::HS256)?;

    let response = router(Some(decoder()))
        .oneshot(
            Request::builder()
                .uri("/whoami")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    assert_eq!(String::from_utf8_lossy(&bytes), tenant.to_string());
    Ok(())
}

#[tokio::test]
async fn jwt_no_authorization_header_is_401() -> Result<(), Box<dyn Error>> {
    let (status, _) = call(router(Some(decoder())), None).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn jwt_authorization_without_bearer_prefix_is_401() -> Result<(), Box<dyn Error>> {
    let token = token_with(&valid_claims(Uuid::new_v4()), SECRET, Algorithm::HS256)?;

    let (status, _) = call(router(Some(decoder())), Some(&token)).await?;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn jwt_wrong_secret_is_401() -> Result<(), Box<dyn Error>> {
    let token = token_with(
        &valid_claims(Uuid::new_v4()),
        "not-the-configured-secret",
        Algorithm::HS256,
    )?;

    let (status, _) = call(router(Some(decoder())), Some(&format!("Bearer {token}"))).await?;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn jwt_wrong_algorithm_is_401() -> Result<(), Box<dyn Error>> {
    // The D2 pin: a token signed with the right secret but a family the
    // decoder does not accept must still be rejected. An unpinned
    // `Validation` is exactly how `alg: none`-style bypasses land.
    let token = token_with(&valid_claims(Uuid::new_v4()), SECRET, Algorithm::HS384)?;

    let (status, _) = call(router(Some(decoder())), Some(&format!("Bearer {token}"))).await?;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn jwt_expired_token_is_401() -> Result<(), Box<dyn Error>> {
    let claims = Claims {
        sub: Uuid::new_v4(),
        exp: (OffsetDateTime::now_utc() - Duration::hours(1)).unix_timestamp() as usize,
    };
    let token = token_with(&claims, SECRET, Algorithm::HS256)?;

    let (status, _) = call(router(Some(decoder())), Some(&format!("Bearer {token}"))).await?;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}

/// `Claims::sub` is a `Uuid`, so a well-formed token can never carry a
/// malformed one -- this fixture is the one place a claims shape not typed
/// as `Claims` is deliberately encoded, to prove `DemoTenant` still rejects.
#[derive(Serialize)]
struct ClaimsWithStringSub {
    sub: &'static str,
    exp: usize,
}

#[tokio::test]
async fn jwt_non_uuid_sub_is_401() -> Result<(), Box<dyn Error>> {
    let claims = ClaimsWithStringSub {
        sub: "not-a-uuid",
        exp: (OffsetDateTime::now_utc() + Duration::hours(1)).unix_timestamp() as usize,
    };
    let token = token_with(&claims, SECRET, Algorithm::HS256)?;

    let (status, _) = call(router(Some(decoder())), Some(&format!("Bearer {token}"))).await?;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn jwt_missing_decoder_extension_is_401() -> Result<(), Box<dyn Error>> {
    let token = token_with(&valid_claims(Uuid::new_v4()), SECRET, Algorithm::HS256)?;

    // No `Extension(JwtDecoder)` layer installed -- even an otherwise-valid
    // token must still be rejected (D3).
    let (status, _) = call(router(None), Some(&format!("Bearer {token}"))).await?;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn jwt_every_rejection_looks_the_same() -> Result<(), Box<dyn Error>> {
    let tenant = Uuid::new_v4();
    let valid = token_with(&valid_claims(tenant), SECRET, Algorithm::HS256)?;
    let wrong_secret = token_with(
        &valid_claims(tenant),
        "not-the-configured-secret",
        Algorithm::HS256,
    )?;
    let wrong_alg = token_with(&valid_claims(tenant), SECRET, Algorithm::HS384)?;
    let expired = token_with(
        &Claims {
            sub: tenant,
            exp: (OffsetDateTime::now_utc() - Duration::hours(1)).unix_timestamp() as usize,
        },
        SECRET,
        Algorithm::HS256,
    )?;
    let bad_sub = token_with(
        &ClaimsWithStringSub {
            sub: "not-a-uuid",
            exp: (OffsetDateTime::now_utc() + Duration::hours(1)).unix_timestamp() as usize,
        },
        SECRET,
        Algorithm::HS256,
    )?;

    let cases = [
        call(router(Some(decoder())), None).await?,
        call(router(Some(decoder())), Some(&valid)).await?, // missing "Bearer "
        call(
            router(Some(decoder())),
            Some(&format!("Bearer {wrong_secret}")),
        )
        .await?,
        call(
            router(Some(decoder())),
            Some(&format!("Bearer {wrong_alg}")),
        )
        .await?,
        call(router(Some(decoder())), Some(&format!("Bearer {expired}"))).await?,
        call(router(Some(decoder())), Some(&format!("Bearer {bad_sub}"))).await?,
        call(router(None), Some(&format!("Bearer {valid}"))).await?,
    ];

    let details: Vec<_> = cases
        .iter()
        .map(|(status, body)| {
            assert_eq!(*status, StatusCode::UNAUTHORIZED);
            body["detail"].clone()
        })
        .collect();
    let first = &details[0];
    assert!(
        details.iter().all(|detail| detail == first),
        "every 401 cause must produce the identical detail: {details:?}"
    );
    Ok(())
}

#[tokio::test]
async fn jwt_rejection_is_problem_json_with_www_authenticate_bearer() {
    let response = ApiError::Unauthorized.into_response();

    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/problem+json")
    );
    assert_eq!(
        response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Bearer")
    );
}
