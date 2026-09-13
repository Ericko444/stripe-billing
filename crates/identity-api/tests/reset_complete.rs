//! `POST /auth/password-reset/complete`: R10, the single invalid-link answer,
//! the headers every answer carries, and the per-IP limit.

use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use audit::CorrelationId;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, Response, StatusCode, header};
use identity_api::{IdentityState, identity_router};
use identity_domain::{Email, NewPassword, Password, TenantId};
use identity_service::{
    ActiveSession, Authentication, CompleteResetError, InMemoryRateLimiter, LoginError,
    LoginOutcome, Me, PasswordChangeError, PasswordResets, ProfileError, SelectTenantError,
    SessionError, SystemClock, TenantSelection, reset_queue,
};
use secrecy::ExposeSecret;
use serde_json::Value;
use tower::ServiceExt;

const GOOD_TOKEN: &str = "good.token";
const NEW_PASSWORD: &str = "a much longer and newer passphrase";
const CLIENT: [u8; 4] = [203, 0, 113, 7];

/// `GOOD_TOKEN` works exactly once; every other token is an invalid link; a
/// new password under the policy is refused before the token is considered.
#[derive(Default)]
struct ScriptedResets {
    used: std::sync::Mutex<bool>,
}

#[async_trait]
impl PasswordResets for ScriptedResets {
    async fn complete_reset(
        &self,
        presented: &str,
        new_password: Password,
        _: CorrelationId,
    ) -> Result<(), CompleteResetError> {
        NewPassword::check(new_password).map_err(CompleteResetError::Policy)?;
        let mut used = self.used.lock().unwrap_or_else(|p| p.into_inner());
        if presented != GOOD_TOKEN || *used {
            return Err(CompleteResetError::InvalidLink);
        }
        *used = true;
        Ok(())
    }
}

/// These tests never touch sessions.
struct NoAuth;

#[async_trait]
impl Authentication for NoAuth {
    async fn login(
        &self,
        _: &Email,
        _: &Password,
        _: CorrelationId,
    ) -> Result<LoginOutcome, LoginError> {
        Err(LoginError::Unavailable("unused".into()))
    }
    async fn authenticate(&self, _: &str) -> Result<ActiveSession, SessionError> {
        Err(SessionError::Unauthenticated)
    }
    async fn me(&self, _: &ActiveSession) -> Result<Me, SessionError> {
        Err(SessionError::Unauthenticated)
    }
    async fn select_tenant(
        &self,
        _: &ActiveSession,
        _: TenantId,
        _: CorrelationId,
    ) -> Result<TenantSelection, SelectTenantError> {
        Err(SelectTenantError::Unauthenticated)
    }
    async fn logout(&self, _: &ActiveSession) -> Result<(), SessionError> {
        Ok(())
    }
    async fn update_display_name(
        &self,
        _: &ActiveSession,
        _: &str,
        _: CorrelationId,
    ) -> Result<Me, ProfileError> {
        Err(ProfileError::Unauthenticated)
    }
    async fn change_password(
        &self,
        _: &ActiveSession,
        _: &Password,
        _: Password,
        _: CorrelationId,
    ) -> Result<(), PasswordChangeError> {
        Err(PasswordChangeError::WrongCurrentPassword)
    }
}

fn app() -> Router {
    identity_router(IdentityState::new(
        Arc::new(NoAuth),
        Arc::new(ScriptedResets::default()),
        Arc::new(NoMembers),
        Arc::new(InMemoryRateLimiter::new(SystemClock)),
        reset_queue(1).0,
    ))
}

fn request(token: &str, new_password: &str) -> Result<Request<Body>, Box<dyn Error>> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/auth/password-reset/complete")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "token": token, "new_password": new_password }).to_string(),
        ))?;
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((CLIENT, 40_000))));
    Ok(request)
}

fn header_value(response: &Response<Body>, name: header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

async fn body_without_id(response: Response<Body>) -> Result<Value, Box<dyn Error>> {
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let mut body: Value = serde_json::from_slice(&bytes)?;
    if let Some(object) = body.as_object_mut() {
        object.remove("correlation_id");
    }
    Ok(body)
}

/// R10 -- a completed reset starts no session. The user logs in with the new
/// password; holding a link is not logging in.
#[tokio::test]
async fn completing_a_reset_sets_no_cookie() -> Result<(), Box<dyn Error>> {
    let response = app().oneshot(request(GOOD_TOKEN, NEW_PASSWORD)?).await?;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    assert_eq!(
        header_value(&response, header::REFERRER_POLICY).as_deref(),
        Some("no-referrer")
    );
    Ok(())
}

#[tokio::test]
async fn every_unusable_link_gets_one_identical_400() -> Result<(), Box<dyn Error>> {
    let app = app();
    let used_once = app
        .clone()
        .oneshot(request(GOOD_TOKEN, NEW_PASSWORD)?)
        .await?;
    assert_eq!(used_once.status(), StatusCode::NO_CONTENT);
    let forged = identity_domain::SplitToken::from_bytes([1; 16], [2; 32])
        .to_wire()
        .expose_secret()
        .to_string();

    let mut bodies = Vec::new();
    for (case, token) in [
        ("already used", GOOD_TOKEN),
        ("malformed", "garbage"),
        ("empty", ""),
        ("well-formed but unknown", forged.as_str()),
    ] {
        let response = app.clone().oneshot(request(token, NEW_PASSWORD)?).await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{case}");
        assert!(
            response.headers().get(header::SET_COOKIE).is_none(),
            "{case}"
        );
        assert_eq!(
            header_value(&response, header::REFERRER_POLICY).as_deref(),
            Some("no-referrer"),
            "{case}"
        );
        bodies.push(body_without_id(response).await?);
    }
    assert!(
        bodies.windows(2).all(|pair| pair[0] == pair[1]),
        "{bodies:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_password_the_policy_refuses_is_422_and_leaves_the_link_usable()
-> Result<(), Box<dyn Error>> {
    let app = app();

    let refused = app.clone().oneshot(request(GOOD_TOKEN, "short")?).await?;
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let then = app.oneshot(request(GOOD_TOKEN, NEW_PASSWORD)?).await?;
    assert_eq!(then.status(), StatusCode::NO_CONTENT);
    Ok(())
}

#[tokio::test]
async fn completions_from_one_ip_are_limited() -> Result<(), Box<dyn Error>> {
    let app = app();
    for attempt in 1..=20 {
        let response = app
            .clone()
            .oneshot(request("garbage", NEW_PASSWORD)?)
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "attempt {attempt}"
        );
    }
    let limited = app.oneshot(request(GOOD_TOKEN, NEW_PASSWORD)?).await?;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    Ok(())
}

/// These tests never manage members.
struct NoMembers;

#[async_trait]
impl identity_service::Members for NoMembers {
    async fn list_members(
        &self,
        _: &identity_service::ActiveSession,
    ) -> Result<Vec<identity_domain::TenantMember>, identity_service::MembersError> {
        Err(identity_service::MembersError::Unavailable(
            "not used by these tests".into(),
        ))
    }

    async fn add_member(
        &self,
        _: &identity_service::ActiveSession,
        _: &identity_domain::Email,
        _: identity_domain::Role,
        _: CorrelationId,
    ) -> Result<identity_domain::TenantMember, identity_service::MembersError> {
        Err(identity_service::MembersError::Unavailable(
            "not used by these tests".into(),
        ))
    }
}
