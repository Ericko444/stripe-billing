//! The origin check: a state-changing request carrying the session cookie
//! must come from an allowed origin. Keyed on the credential, not the path.

use std::error::Error;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::middleware;
use axum::routing::any;
use identity_api::{AllowedOrigins, origin_check};
use tower::ServiceExt;

const APP: &str = "http://localhost:5173";
const SESSION: &str = "__Host-session=aa.bb";

fn router() -> Router {
    Router::new()
        .route("/anything", any(|| async { "reached" }))
        .layer(middleware::from_fn_with_state(
            AllowedOrigins::new([APP]),
            origin_check,
        ))
}

async fn status(
    method: Method,
    cookie: Option<&str>,
    origin: Option<&str>,
) -> Result<StatusCode, Box<dyn Error>> {
    let mut request = Request::builder().method(method).uri("/anything");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    let response = router().oneshot(request.body(Body::empty())?).await?;
    Ok(response.status())
}

#[tokio::test]
async fn a_cookie_carrying_write_from_a_foreign_origin_is_403() -> Result<(), Box<dyn Error>> {
    for method in [Method::POST, Method::PATCH, Method::PUT, Method::DELETE] {
        assert_eq!(
            status(method.clone(), Some(SESSION), Some("https://evil.example")).await?,
            StatusCode::FORBIDDEN,
            "{method}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_cookie_carrying_write_with_no_origin_is_403() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        status(Method::POST, Some(SESSION), None).await?,
        StatusCode::FORBIDDEN
    );
    Ok(())
}

#[tokio::test]
async fn a_lookalike_origin_is_not_the_allowed_one() -> Result<(), Box<dyn Error>> {
    for origin in [
        "http://localhost:5173.evil.example",
        "https://localhost:5173",
        "http://localhost:5174",
        "null",
    ] {
        assert_eq!(
            status(Method::POST, Some(SESSION), Some(origin)).await?,
            StatusCode::FORBIDDEN,
            "{origin}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_cookie_carrying_write_from_the_app_passes() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        status(Method::POST, Some(SESSION), Some(APP)).await?,
        StatusCode::OK
    );
    Ok(())
}

/// The Stripe webhook's shape: a `POST` with no cookie and no `Origin`. It
/// needs no carve-out, because the rule is about the credential.
#[tokio::test]
async fn a_write_without_the_session_cookie_passes() -> Result<(), Box<dyn Error>> {
    assert_eq!(status(Method::POST, None, None).await?, StatusCode::OK);
    assert_eq!(
        status(
            Method::POST,
            Some("theme=dark"),
            Some("https://evil.example")
        )
        .await?,
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn reads_pass_with_the_cookie_and_no_origin() -> Result<(), Box<dyn Error>> {
    for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
        assert_eq!(
            status(method.clone(), Some(SESSION), None).await?,
            StatusCode::OK,
            "{method}"
        );
    }
    Ok(())
}
