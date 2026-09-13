//! `POST /auth/login`, `GET /auth/me` and `AuthenticatedSession`, through the
//! real router against a scripted `Authentication`.

use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use audit::CorrelationId;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, Response, StatusCode, header};
use axum::routing::get;
use identity_api::{AuthenticatedSession, IdentityState, identity_router};
use identity_domain::{
    Email, Membership, MembershipId, Password, Role, SessionId, SessionTenant, SplitToken,
    TenantId, UserId,
};
use identity_service::{
    ActiveSession, Authentication, InMemoryRateLimiter, LoginError, LoginOutcome, Me,
    PasswordChangeError, ProfileError, SelectTenantError, SessionError, SessionScope, SystemClock,
    TenantSelection,
};
use secrecy::ExposeSecret;
use serde_json::Value;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct horse battery staple";
const TOKEN_SELECTOR: [u8; 16] = [0xab; 16];
const TOKEN_VERIFIER: [u8; 32] = [0xcd; 32];

fn alice() -> UserId {
    UserId::new(Uuid::from_u128(1))
}

fn tenant_a() -> Membership {
    Membership {
        id: MembershipId::new(Uuid::from_u128(10)),
        user_id: alice(),
        tenant_id: TenantId::new(Uuid::from_u128(100)),
        tenant_name: "Tenant A".to_string(),
        role: Role::Owner,
    }
}

fn valid_wire() -> String {
    SplitToken::from_bytes(TOKEN_SELECTOR, TOKEN_VERIFIER)
        .to_wire()
        .expose_secret()
        .to_string()
}

fn expires_at() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + Duration::days(20_000)
}

/// Alice with one membership logs in with `PASSWORD`; `valid_wire()` is her
/// session; anything else is refused. `outage` makes every call fail as
/// unavailable.
struct ScriptedAuth {
    outage: bool,
}

#[async_trait]
impl Authentication for ScriptedAuth {
    async fn login(
        &self,
        email: &Email,
        password: &Password,
        _correlation_id: CorrelationId,
    ) -> Result<LoginOutcome, LoginError> {
        if self.outage {
            return Err(LoginError::Unavailable("database is down".into()));
        }
        if email.as_str() != "alice@example.test" || password.expose_secret() != PASSWORD {
            return Err(LoginError::InvalidCredentials);
        }
        Ok(LoginOutcome {
            token: SplitToken::from_bytes(TOKEN_SELECTOR, TOKEN_VERIFIER),
            user_id: alice(),
            scope: SessionScope::Tenant(tenant_a()),
            memberships: vec![tenant_a()],
            expires_at: expires_at(),
        })
    }

    async fn authenticate(&self, presented: &str) -> Result<ActiveSession, SessionError> {
        if self.outage {
            return Err(SessionError::Unavailable("database is down".into()));
        }
        if presented != valid_wire() {
            return Err(SessionError::Unauthenticated);
        }
        Ok(ActiveSession {
            id: SessionId::new(Uuid::from_u128(1000)),
            user_id: alice(),
            tenant: Some(SessionTenant {
                tenant_id: tenant_a().tenant_id,
                role: Role::Owner,
            }),
            authenticated_at: expires_at() - Duration::hours(8),
            expires_at: expires_at(),
        })
    }

    async fn me(&self, session: &ActiveSession) -> Result<Me, SessionError> {
        Ok(Me {
            user_id: session.user_id,
            email: Email::parse("alice@example.test").map_err(|_| SessionError::Unauthenticated)?,
            display_name: "Alice".to_string(),
            tenant: session.tenant,
            memberships: vec![tenant_a()],
            expires_at: session.expires_at,
        })
    }

    async fn select_tenant(
        &self,
        _session: &ActiveSession,
        tenant_id: TenantId,
        _correlation_id: CorrelationId,
    ) -> Result<TenantSelection, SelectTenantError> {
        if tenant_id != tenant_a().tenant_id {
            return Err(SelectTenantError::NotAMember);
        }
        Ok(TenantSelection {
            token: SplitToken::from_bytes(ROTATED_SELECTOR, ROTATED_VERIFIER),
            membership: tenant_a(),
            memberships: vec![tenant_a()],
            expires_at: expires_at(),
            max_age: Duration::hours(1),
        })
    }

    async fn logout(&self, _session: &ActiveSession) -> Result<(), SessionError> {
        Ok(())
    }

    async fn change_password(
        &self,
        _session: &ActiveSession,
        current: &Password,
        new: Password,
        _correlation_id: CorrelationId,
    ) -> Result<(), PasswordChangeError> {
        identity_domain::NewPassword::check(new).map_err(PasswordChangeError::Policy)?;
        if current.expose_secret() != PASSWORD {
            return Err(PasswordChangeError::WrongCurrentPassword);
        }
        Ok(())
    }

    async fn update_display_name(
        &self,
        session: &ActiveSession,
        display_name: &str,
        _correlation_id: CorrelationId,
    ) -> Result<Me, ProfileError> {
        let name = identity_domain::DisplayName::parse(display_name)
            .map_err(|_| ProfileError::InvalidDisplayName)?;
        let mut me = self
            .me(session)
            .await
            .map_err(|_| ProfileError::Unauthenticated)?;
        me.display_name = name.as_str().to_string();
        Ok(me)
    }
}

const ROTATED_SELECTOR: [u8; 16] = [0x11; 16];
const ROTATED_VERIFIER: [u8; 32] = [0x22; 32];

fn state(outage: bool) -> IdentityState {
    IdentityState::new(
        Arc::new(ScriptedAuth { outage }),
        Arc::new(NoResets),
        Arc::new(NoMembers),
        Arc::new(InMemoryRateLimiter::new(SystemClock)),
        identity_service::reset_queue(8).0,
    )
}

fn router() -> Router {
    identity_router(state(false))
}

fn login_request(body: &str) -> Result<Request<Body>, Box<dyn Error>> {
    login_request_from(body, CLIENT)
}

const CLIENT: [u8; 4] = [203, 0, 113, 7];

/// A login request as `axum::serve` with connect info would hand it over:
/// with the socket peer in the extensions.
fn login_request_from(body: &str, peer: [u8; 4]) -> Result<Request<Body>, Box<dyn Error>> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))?;
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((peer, 50_000))));
    Ok(request)
}

fn me_request(cookie: Option<&str>) -> Result<Request<Body>, Box<dyn Error>> {
    let mut builder = Request::builder().uri("/auth/me");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    Ok(builder.body(Body::empty())?)
}

async fn json(response: Response<Body>) -> Result<Value, Box<dyn Error>> {
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// A problem body with its per-request id removed, so two refusals can be
/// compared byte for byte.
async fn problem_without_id(response: Response<Body>) -> Result<Value, Box<dyn Error>> {
    let mut body = json(response).await?;
    if let Some(object) = body.as_object_mut() {
        object.remove("correlation_id");
    }
    Ok(body)
}

fn login_body(email: &str, password: &str) -> String {
    serde_json::json!({ "email": email, "password": password }).to_string()
}

#[tokio::test]
async fn login_sets_the_session_cookie_and_keeps_the_token_out_of_the_body()
-> Result<(), Box<dyn Error>> {
    let response = router()
        .oneshot(login_request(&login_body("Alice@Example.test", PASSWORD))?)
        .await?;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        cookie,
        format!(
            "__Host-session={}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=28800",
            valid_wire()
        )
    );

    let body = json(response).await?;
    assert!(!body.to_string().contains(&valid_wire()));
    assert_eq!(body["tenant"]["tenant_name"], "Tenant A");
    assert_eq!(body["tenant"]["role"], "owner");
    assert_eq!(body["memberships"][0]["tenant_name"], "Tenant A");
    assert_eq!(body["expires_at"], "2024-10-04T00:00:00Z");
    Ok(())
}

#[tokio::test]
async fn every_login_refusal_is_the_same_401_and_sets_no_cookie() -> Result<(), Box<dyn Error>> {
    let mut bodies = Vec::new();
    for (email, password) in [
        ("alice@example.test", "wrong horse battery staple"),
        ("nobody@example.test", PASSWORD),
        ("not an address", PASSWORD),
    ] {
        let response = router()
            .oneshot(login_request(&login_body(email, password))?)
            .await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{email}");
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        bodies.push(problem_without_id(response).await?);
    }

    assert!(bodies.windows(2).all(|pair| pair[0] == pair[1]));
    Ok(())
}

#[tokio::test]
async fn a_body_that_does_not_parse_is_400() -> Result<(), Box<dyn Error>> {
    for body in ["", "{", r#"{"email":"alice@example.test"}"#] {
        let response = router().oneshot(login_request(body)?).await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body:?}");
    }
    Ok(())
}

#[tokio::test]
async fn an_outage_is_a_500_that_says_nothing_about_it() -> Result<(), Box<dyn Error>> {
    let response = identity_router(state(true))
        .oneshot(login_request(&login_body("alice@example.test", PASSWORD))?)
        .await?;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!json(response).await?.to_string().contains("database"));
    Ok(())
}

#[tokio::test]
async fn me_returns_the_callers_account_for_a_valid_session() -> Result<(), Box<dyn Error>> {
    let cookie = format!("theme=dark; __Host-session={}", valid_wire());
    let response = router().oneshot(me_request(Some(&cookie))?).await?;

    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await?;
    assert_eq!(body["email"], "alice@example.test");
    assert_eq!(
        body["tenant"]["tenant_id"],
        Uuid::from_u128(100).to_string()
    );
    assert_eq!(body["tenant"]["role"], "owner");
    Ok(())
}

#[tokio::test]
async fn every_refused_session_is_the_same_401() -> Result<(), Box<dyn Error>> {
    let mut bodies = Vec::new();
    for cookie in [
        None,
        Some("__Host-session="),
        Some("__Host-session=garbage"),
        Some("session=aa.bb"),
        Some(
            "__Host-session=abababababababababababababababab.0000000000000000000000000000000000000000000000000000000000000000",
        ),
    ] {
        let response = router().oneshot(me_request(cookie)?).await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{cookie:?}");
        bodies.push(problem_without_id(response).await?);
    }

    assert!(bodies.windows(2).all(|pair| pair[0] == pair[1]));
    Ok(())
}

/// The property `demo` relies on: the extractor works on a router whose
/// state type this crate has never seen.
#[tokio::test]
async fn the_extractor_works_under_a_foreign_state_type() -> Result<(), Box<dyn Error>> {
    #[derive(Clone)]
    struct SomeoneElsesState;

    async fn whoami(session: AuthenticatedSession) -> String {
        session
            .tenant_id()
            .map(|tenant| tenant.as_uuid().to_string())
            .unwrap_or_default()
    }

    let router: Router = Router::new()
        .route("/whoami", get(whoami))
        .with_state(SomeoneElsesState)
        .layer(axum::Extension(state(false)));

    let authorised = router
        .clone()
        .oneshot(me_request_to(
            "/whoami",
            Some(&format!("__Host-session={}", valid_wire())),
        )?)
        .await?;
    assert_eq!(authorised.status(), StatusCode::OK);
    let bytes = to_bytes(authorised.into_body(), usize::MAX).await?;
    assert_eq!(bytes, Uuid::from_u128(100).to_string().as_bytes());

    let refused = router.oneshot(me_request_to("/whoami", None)?).await?;
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn a_router_missing_the_state_extension_fails_loudly() -> Result<(), Box<dyn Error>> {
    async fn whoami(_session: AuthenticatedSession) -> &'static str {
        "unreachable"
    }
    let router: Router = Router::new().route("/whoami", get(whoami));

    let response = router
        .oneshot(me_request_to(
            "/whoami",
            Some(&format!("__Host-session={}", valid_wire())),
        )?)
        .await?;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    Ok(())
}

fn me_request_to(uri: &str, cookie: Option<&str>) -> Result<Request<Body>, Box<dyn Error>> {
    let mut builder = Request::builder().uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    Ok(builder.body(Body::empty())?)
}

fn post_json(uri: &str, cookie: Option<&str>, body: &str) -> Result<Request<Body>, Box<dyn Error>> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    Ok(builder.body(Body::from(body.to_string()))?)
}

fn set_cookie(response: &Response<Body>) -> Option<String> {
    response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

#[tokio::test]
async fn selecting_a_tenant_answers_with_a_rotated_cookie_for_the_remaining_lifetime()
-> Result<(), Box<dyn Error>> {
    let body = serde_json::json!({ "tenant_id": tenant_a().tenant_id.as_uuid() }).to_string();
    let cookie = format!("__Host-session={}", valid_wire());

    let response = router()
        .oneshot(post_json("/auth/tenant", Some(&cookie), &body)?)
        .await?;

    assert_eq!(response.status(), StatusCode::OK);
    let rotated = SplitToken::from_bytes(ROTATED_SELECTOR, ROTATED_VERIFIER);
    assert_eq!(
        set_cookie(&response),
        Some(format!(
            "__Host-session={}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=3600",
            rotated.to_wire().expose_secret()
        ))
    );
    assert_eq!(json(response).await?["tenant"]["tenant_name"], "Tenant A");
    Ok(())
}

#[tokio::test]
async fn selecting_a_tenant_one_is_not_a_member_of_is_404() -> Result<(), Box<dyn Error>> {
    let body = serde_json::json!({ "tenant_id": Uuid::from_u128(999) }).to_string();
    let cookie = format!("__Host-session={}", valid_wire());

    let response = router()
        .oneshot(post_json("/auth/tenant", Some(&cookie), &body)?)
        .await?;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(set_cookie(&response).is_none());
    Ok(())
}

#[tokio::test]
async fn selecting_a_tenant_needs_a_session() -> Result<(), Box<dyn Error>> {
    let body = serde_json::json!({ "tenant_id": tenant_a().tenant_id.as_uuid() }).to_string();
    let response = router()
        .oneshot(post_json("/auth/tenant", None, &body)?)
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn logout_clears_the_cookie_with_or_without_a_live_session() -> Result<(), Box<dyn Error>> {
    let live = format!("__Host-session={}", valid_wire());
    for cookie in [
        Some(live.as_str()),
        Some("__Host-session=stale.token"),
        None,
    ] {
        let response = router()
            .oneshot(post_json("/auth/logout", cookie, "")?)
            .await?;
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "{cookie:?}");
        assert_eq!(
            set_cookie(&response).as_deref(),
            Some("__Host-session=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0")
        );
    }
    Ok(())
}

fn patch_me(cookie: Option<&str>, body: &str) -> Result<Request<Body>, Box<dyn Error>> {
    let mut builder = Request::builder()
        .method("PATCH")
        .uri("/auth/me")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    Ok(builder.body(Body::from(body.to_string()))?)
}

#[tokio::test]
async fn patching_me_returns_the_updated_account() -> Result<(), Box<dyn Error>> {
    let cookie = format!("__Host-session={}", valid_wire());
    let response = router()
        .oneshot(patch_me(
            Some(&cookie),
            r#"{"display_name":"  Alice L. "}"#,
        )?)
        .await?;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json(response).await?["display_name"], "Alice L.");
    Ok(())
}

#[tokio::test]
async fn patching_me_with_an_invalid_name_is_400_and_without_a_session_401()
-> Result<(), Box<dyn Error>> {
    let cookie = format!("__Host-session={}", valid_wire());
    let invalid = router()
        .oneshot(patch_me(Some(&cookie), r#"{"display_name":"a\nb"}"#)?)
        .await?;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    let anonymous = router()
        .oneshot(patch_me(None, r#"{"display_name":"Alice"}"#)?)
        .await?;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

/// The eleventh attempt for one address from one IP inside the window is
/// refused before the service runs; the same IP trying another address is
/// not.
#[tokio::test]
async fn the_eleventh_login_for_one_address_from_one_ip_is_429() -> Result<(), Box<dyn Error>> {
    let app = router();
    let wrong = login_body("alice@example.test", "wrong horse battery staple");

    for attempt in 1..=10 {
        let response = app.clone().oneshot(login_request(&wrong)?).await?;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "attempt {attempt}"
        );
    }

    let limited = app.clone().oneshot(login_request(&wrong)?).await?;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after = limited
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or_default();
    assert!((1..=900).contains(&retry_after), "{retry_after}");

    // Even the right password is refused while the window lasts...
    let right = app
        .clone()
        .oneshot(login_request(&login_body("alice@example.test", PASSWORD))?)
        .await?;
    assert_eq!(right.status(), StatusCode::TOO_MANY_REQUESTS);

    // ...but another address from the same IP, and the same address from
    // another IP, are counted separately.
    let other_address = app
        .clone()
        .oneshot(login_request(&login_body("bob@example.test", PASSWORD))?)
        .await?;
    assert_eq!(other_address.status(), StatusCode::UNAUTHORIZED);
    let other_ip = app
        .oneshot(login_request_from(
            &login_body("alice@example.test", PASSWORD),
            [198, 51, 100, 1],
        )?)
        .await?;
    assert_eq!(other_ip.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn a_login_served_without_connect_info_fails_loudly() -> Result<(), Box<dyn Error>> {
    let request = Request::builder()
        .method("POST")
        .uri("/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(login_body("alice@example.test", PASSWORD)))?;

    let response = router().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    Ok(())
}

#[tokio::test]
async fn changing_the_password_answers_by_cause() -> Result<(), Box<dyn Error>> {
    let cookie = format!("__Host-session={}", valid_wire());
    let body = |current: &str, new: &str| {
        serde_json::json!({ "current_password": current, "new_password": new }).to_string()
    };
    let new = "a much longer and newer passphrase";

    for (case, session, request, expected) in [
        (
            "changed",
            Some(cookie.as_str()),
            body(PASSWORD, new),
            StatusCode::NO_CONTENT,
        ),
        (
            "wrong current password",
            Some(cookie.as_str()),
            body("wrong horse battery staple", new),
            StatusCode::FORBIDDEN,
        ),
        (
            "new password too short",
            Some(cookie.as_str()),
            body(PASSWORD, "short"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "no session",
            None,
            body(PASSWORD, new),
            StatusCode::UNAUTHORIZED,
        ),
    ] {
        let response = router()
            .oneshot(post_json("/auth/password/change", session, &request)?)
            .await?;
        assert_eq!(response.status(), expected, "{case}");
    }
    Ok(())
}

/// These tests never complete a reset; the double says so if one does.
struct NoResets;

#[async_trait]
impl identity_service::PasswordResets for NoResets {
    async fn complete_reset(
        &self,
        _: &str,
        _: Password,
        _: CorrelationId,
    ) -> Result<(), identity_service::CompleteResetError> {
        Err(identity_service::CompleteResetError::Unavailable(
            "not used by these tests".into(),
        ))
    }
}

/// These tests never manage members; the double says so if one does.
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
