//! `DemoTenant`'s full rejection-path suite, exercised against a minimal
//! router so nothing here depends on `main`'s full mount.
//!
//! "The token was refused" is worthless if it was refused for the wrong
//! reason, so every rejection path gets its own named test -- and the row
//! that matters most, [`jwt_every_rejection_looks_the_same`], checks that all
//! seven produce the byte-identical `detail`: the response alone must
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
use common::{CapturedLogs, unused_state};
use demo::jwt::{Claims, DemoTenant, JwtDecoder};
use demo::token::demo_token_router;
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header as JwtHeader, Validation, decode, encode,
};
use secrecy::SecretString;
use serde::Serialize;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "the-demo-signing-secret";

async fn whoami(tenant: DemoTenant) -> impl IntoResponse {
    let tenant_id: domain::TenantId = tenant.into();
    tenant_id.as_uuid().to_string()
}

/// A router with exactly one route behind `DemoTenant`, mirroring how
/// `main` mounts `billing_router::<DemoTenant>` -- except this exercises the
/// extractor alone.
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

/// `/whoami` (behind `DemoTenant`) merged with the real `/demo/token` mint
/// route over the same secret -- the composition `main` builds, so a
/// round-trip test here exercises the actual seam rather than a stand-in.
fn mounted_app(secret: &SecretString) -> Router {
    Router::new()
        .route("/whoami", get(whoami))
        .with_state(unused_state())
        .merge(demo_token_router(secret))
        .layer(Extension(JwtDecoder::new(secret)))
}

/// Posts `body` as JSON to `/demo/token` and returns the parsed response.
async fn post_token(app: Router, body: Value) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let request = Request::builder()
        .method("POST")
        .uri("/demo/token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body)?))?;
    let response = app.oneshot(request).await?;

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
    // The HS256 pin: a token signed with the right secret but a family the
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
    // token must still be rejected.
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

#[tokio::test]
async fn token_mint_returns_a_token_and_expiry_about_one_hour_ahead() -> Result<(), Box<dyn Error>>
{
    let secret = SecretString::from(SECRET.to_string());
    let tenant = Uuid::new_v4();

    let (status, body) = post_token(
        mounted_app(&secret),
        json!({ "tenant_id": tenant.to_string() }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK);
    let token = body["token"].as_str().ok_or("missing token")?;
    assert!(!token.is_empty());
    assert!(body["expires_at"].as_str().is_some_and(|s| !s.is_empty()));

    // "About one hour ahead": decode the token's own `exp` and check it
    // against `now + 1h` within a generous tolerance, rather than trying to
    // parse `expires_at` back out of RFC 3339 with no `time` parsing feature
    // in the workspace.
    let claims = decode::<Claims>(
        token,
        &DecodingKey::from_secret(SECRET.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )?
    .claims;
    let expected = (OffsetDateTime::now_utc() + Duration::hours(1)).unix_timestamp();
    let actual = claims.exp as i64;
    assert!(
        (actual - expected).abs() < 30,
        "expected exp near {expected}, got {actual}"
    );
    Ok(())
}

#[tokio::test]
async fn token_malformed_tenant_id_is_400_not_500() -> Result<(), Box<dyn Error>> {
    let secret = SecretString::from(SECRET.to_string());

    let (status, body) =
        post_token(mounted_app(&secret), json!({ "tenant_id": "not-a-uuid" })).await?;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["status"], 400);
    Ok(())
}

#[tokio::test]
async fn token_round_trip_is_accepted_by_demo_tenant() -> Result<(), Box<dyn Error>> {
    let secret = SecretString::from(SECRET.to_string());
    let tenant = Uuid::new_v4();

    let (mint_status, minted) = post_token(
        mounted_app(&secret),
        json!({ "tenant_id": tenant.to_string() }),
    )
    .await?;
    assert_eq!(mint_status, StatusCode::OK);
    let token = minted["token"].as_str().ok_or("missing token")?;

    let response = mounted_app(&secret)
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
async fn token_never_logs_the_minted_token() -> Result<(), Box<dyn Error>> {
    let secret = SecretString::from(SECRET.to_string());
    let tenant = Uuid::new_v4();

    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let (status, body) = {
        let _guard = tracing::subscriber::set_default(subscriber);
        post_token(
            mounted_app(&secret),
            json!({ "tenant_id": tenant.to_string() }),
        )
        .await?
    };

    assert_eq!(status, StatusCode::OK);
    let token = body["token"].as_str().ok_or("missing token")?;
    let logged = logs.contents();
    assert!(
        !logged.contains(token),
        "the minted token must never reach a log line; captured: {logged}"
    );
    Ok(())
}
