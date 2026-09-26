//! `POST /auth/password-reset/request`: R7, R8 and R11.

use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use audit::CorrelationId;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use identity_api::{IdentityState, identity_router};
use identity_domain::{Email, Password, TenantId};
use identity_service::{
    ActiveSession, Authentication, CompleteResetError, InMemoryRateLimiter, LoginError,
    LoginOutcome, Me, PasswordChangeError, PasswordResets, ProfileError, ResetReceiver,
    SelectTenantError, SessionError, SystemClock, TenantSelection, reset_queue,
};
use tower::ServiceExt;

/// Both use-case façades, counting every call and answering none. The reset
/// request handler must never make one: it has no business knowing anything
/// about accounts.
#[derive(Default)]
struct CountingAuth {
    calls: AtomicUsize,
}

impl CountingAuth {
    fn called(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl Authentication for CountingAuth {
    async fn login(
        &self,
        _: &Email,
        _: &Password,
        _: CorrelationId,
    ) -> Result<LoginOutcome, LoginError> {
        self.called();
        Err(LoginError::Unavailable("counting double".into()))
    }
    async fn authenticate(&self, _: &str) -> Result<ActiveSession, SessionError> {
        self.called();
        Err(SessionError::Unauthenticated)
    }
    async fn me(&self, _: &ActiveSession) -> Result<Me, SessionError> {
        self.called();
        Err(SessionError::Unauthenticated)
    }
    async fn select_tenant(
        &self,
        _: &ActiveSession,
        _: TenantId,
        _: CorrelationId,
    ) -> Result<TenantSelection, SelectTenantError> {
        self.called();
        Err(SelectTenantError::Unauthenticated)
    }
    async fn logout(&self, _: &ActiveSession) -> Result<(), SessionError> {
        self.called();
        Ok(())
    }
    async fn update_display_name(
        &self,
        _: &ActiveSession,
        _: &str,
        _: CorrelationId,
    ) -> Result<Me, ProfileError> {
        self.called();
        Err(ProfileError::Unauthenticated)
    }
    async fn change_password(
        &self,
        _: &ActiveSession,
        _: &Password,
        _: Password,
        _: CorrelationId,
    ) -> Result<(), PasswordChangeError> {
        self.called();
        Err(PasswordChangeError::WrongCurrentPassword)
    }
}

#[async_trait]
impl PasswordResets for CountingAuth {
    async fn complete_reset(
        &self,
        _: &str,
        _: Password,
        _: CorrelationId,
    ) -> Result<(), CompleteResetError> {
        self.called();
        Err(CompleteResetError::InvalidLink)
    }
}

#[async_trait]
impl identity_service::Members for CountingAuth {
    async fn list_members(
        &self,
        _: &ActiveSession,
    ) -> Result<Vec<identity_domain::TenantMember>, identity_service::MembersError> {
        self.called();
        Ok(Vec::new())
    }

    async fn add_member(
        &self,
        _: &ActiveSession,
        _: &identity_domain::Email,
        _: identity_domain::Role,
        _: CorrelationId,
    ) -> Result<identity_domain::TenantMember, identity_service::MembersError> {
        self.called();
        Err(identity_service::MembersError::Forbidden)
    }

    async fn suspend_member(
        &self,
        _: &ActiveSession,
        _: identity_domain::MembershipId,
        _: CorrelationId,
    ) -> Result<(), identity_service::MembersError> {
        self.called();
        Err(identity_service::MembersError::Forbidden)
    }
}

struct World {
    app: Router,
    auth: Arc<CountingAuth>,
    receiver: ResetReceiver,
}

fn world() -> World {
    let auth = Arc::new(CountingAuth::default());
    let (queue, receiver) = reset_queue(64);
    let state = IdentityState::new(
        auth.clone(),
        auth.clone(),
        auth.clone(),
        Arc::new(NoDeactivations),
        Arc::new(InMemoryRateLimiter::new(SystemClock)),
        queue,
    );
    World {
        app: identity_router(state),
        auth,
        receiver,
    }
}

fn request(email: &str, peer: [u8; 4]) -> Result<Request<Body>, Box<dyn Error>> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/auth/password-reset/request")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "email": email }).to_string(),
        ))?;
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((peer, 40_000))));
    Ok(request)
}

const CLIENT: [u8; 4] = [203, 0, 113, 7];

fn drain(receiver: &mut ResetReceiver) -> Vec<String> {
    let mut queued = Vec::new();
    while let Some(job) = receiver.try_recv() {
        queued.push(job.email.as_str().to_string());
    }
    queued
}

/// R7 -- the response is byte-identical whether or not the address has an
/// account. Here that is true by construction: nothing on the request path
/// can know. The two addresses are one the seed creates and one nobody has.
#[tokio::test]
async fn reset_request_body_is_identical_for_known_and_unknown_addresses()
-> Result<(), Box<dyn Error>> {
    let w = world();

    let mut answers = Vec::new();
    for address in ["alice@example.test", "nobody@example.test"] {
        let response = w.app.clone().oneshot(request(address, CLIENT)?).await?;
        let status = response.status();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        answers.push((status, headers, body));
    }

    assert_eq!(answers[0].0, StatusCode::ACCEPTED);
    assert_eq!(answers[0], answers[1]);
    Ok(())
}

/// R8 -- the request path does no work that depends on the address having
/// an account, so response time cannot reveal it. Asserted structurally: the
/// request is answered, the address is queued, and not one use case -- the
/// only way to reach account data -- was called.
#[tokio::test]
async fn reset_request_handler_cannot_reach_the_user_repository() -> Result<(), Box<dyn Error>> {
    let mut w = world();

    let response = w
        .app
        .clone()
        .oneshot(request("alice@example.test", CLIENT)?)
        .await?;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(w.auth.calls.load(Ordering::SeqCst), 0);
    assert_eq!(drain(&mut w.receiver), vec!["alice@example.test"]);
    Ok(())
}

/// R11, per address -- over the limit the request is dropped silently: the
/// same `202` and body, nothing queued. And the limit applies to an address
/// with no account exactly as to one with an account, so it is no oracle.
#[tokio::test]
async fn address_limit_is_silent_and_applies_to_unknown_addresses() -> Result<(), Box<dyn Error>> {
    let mut w = world();

    for address in ["nobody@example.test", "alice@example.test"] {
        for attempt in 1..=4 {
            // A different IP each time, so only the address limit is in play.
            let response = w
                .app
                .clone()
                .oneshot(request(address, [198, 51, 100, attempt])?)
                .await?;
            assert_eq!(
                response.status(),
                StatusCode::ACCEPTED,
                "{address} #{attempt}"
            );
        }
    }

    let queued = drain(&mut w.receiver);
    assert_eq!(
        queued
            .iter()
            .filter(|a| *a == "nobody@example.test")
            .count(),
        3
    );
    assert_eq!(
        queued.iter().filter(|a| *a == "alice@example.test").count(),
        3
    );
    Ok(())
}

/// R11, per IP -- one source working through many addresses gets a `429`.
#[tokio::test]
async fn ip_limit_returns_429() -> Result<(), Box<dyn Error>> {
    let w = world();

    for n in 1..=20 {
        let response = w
            .app
            .clone()
            .oneshot(request(&format!("user{n}@example.test"), CLIENT)?)
            .await?;
        assert_eq!(response.status(), StatusCode::ACCEPTED, "request {n}");
    }
    let limited = w
        .app
        .clone()
        .oneshot(request("user21@example.test", CLIENT)?)
        .await?;

    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.headers().get(header::RETRY_AFTER).is_some());
    Ok(())
}

#[tokio::test]
async fn an_address_that_does_not_parse_is_400() -> Result<(), Box<dyn Error>> {
    let w = world();
    let response = w
        .app
        .clone()
        .oneshot(request("not an address", CLIENT)?)
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

/// Deactivation is not exercised by these tests; every call is an error
/// rather than a silent success, so a route that reached it would fail
/// loudly.
struct NoDeactivations;

#[async_trait]
impl identity_service::Deactivations for NoDeactivations {
    async fn deactivate_member(
        &self,
        _: &identity_service::ActiveSession,
        _: identity_domain::MembershipId,
        _: CorrelationId,
    ) -> Result<(), identity_service::MembersError> {
        Err(identity_service::MembersError::Unavailable(
            "not used by these tests".into(),
        ))
    }

    async fn reactivate_member(
        &self,
        _: &identity_service::ActiveSession,
        _: identity_domain::MembershipId,
        _: CorrelationId,
    ) -> Result<(), identity_service::MembersError> {
        Err(identity_service::MembersError::Unavailable(
            "not used by these tests".into(),
        ))
    }

    async fn deactivate_self(
        &self,
        _: &identity_service::ActiveSession,
        _: CorrelationId,
    ) -> Result<(), identity_service::MembersError> {
        Err(identity_service::MembersError::Unavailable(
            "not used by these tests".into(),
        ))
    }
}
