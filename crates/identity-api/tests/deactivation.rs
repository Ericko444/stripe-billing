//! `POST /tenant/members/{id}/deactivate`, `.../reactivate` and
//! `POST /auth/deactivate` through the real router, over a scripted use
//! case: who may call them, what a refusal looks like, and the cookie that
//! self-deactivation clears.
//!
//! The *rule* -- that a tenant may not end an account living elsewhere -- is
//! the service's, and is pinned by `identity-service`'s own tests. What is
//! pinned here is the wire: that its refusal is rendered exactly like every
//! other refusal, so the boundary does not leak what the service was careful
//! not to say.

use std::error::Error;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use audit::CorrelationId;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use identity_api::{IdentityState, identity_router};
use identity_domain::{
    Email, MembershipId, Password, Role, SessionId, SessionTenant, TenantId, UserId,
};
use identity_service::{
    ActiveSession, Authentication, CompleteResetError, Deactivations, InMemoryRateLimiter,
    LoginError, LoginOutcome, Me, MembersError, PasswordChangeError, PasswordResets, ProfileError,
    SelectTenantError, SessionError, SystemClock, TenantSelection, reset_queue,
};
use serde_json::Value;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

fn tenant_a() -> TenantId {
    TenantId::new(Uuid::from_u128(0xa))
}

fn caller() -> UserId {
    UserId::new(Uuid::from_u128(0xca))
}

fn membership() -> MembershipId {
    MembershipId::new(Uuid::from_u128(0x77))
}

/// The cookie value is the caller, as in the members route tests.
struct RoleAuth;

#[async_trait]
impl Authentication for RoleAuth {
    async fn authenticate(&self, presented: &str) -> Result<ActiveSession, SessionError> {
        let tenant = match presented {
            "owner" => Some(Role::Owner),
            "admin" => Some(Role::Admin),
            "member" => Some(Role::Member),
            "unscoped" => None,
            _ => return Err(SessionError::Unauthenticated),
        }
        .map(|role| SessionTenant {
            tenant_id: tenant_a(),
            role,
        });
        let now = OffsetDateTime::now_utc();
        Ok(ActiveSession {
            id: SessionId::new(Uuid::new_v4()),
            user_id: caller(),
            tenant,
            authenticated_at: now,
            expires_at: now + Duration::hours(8),
        })
    }

    async fn login(
        &self,
        _: &Email,
        _: &Password,
        _: CorrelationId,
    ) -> Result<LoginOutcome, LoginError> {
        Err(LoginError::InvalidCredentials)
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
        Err(SelectTenantError::NotAMember)
    }

    async fn logout(&self, _: &ActiveSession) -> Result<(), SessionError> {
        Ok(())
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

    async fn update_display_name(
        &self,
        _: &ActiveSession,
        _: &str,
        _: CorrelationId,
    ) -> Result<Me, ProfileError> {
        Err(ProfileError::Unauthenticated)
    }
}

struct NoResets;

#[async_trait]
impl PasswordResets for NoResets {
    async fn complete_reset(
        &self,
        _: &str,
        _: Password,
        _: CorrelationId,
    ) -> Result<(), CompleteResetError> {
        Err(CompleteResetError::InvalidLink)
    }
}

struct NoMembers;

#[async_trait]
impl identity_service::Members for NoMembers {
    async fn list_members(
        &self,
        _: &ActiveSession,
    ) -> Result<Vec<identity_domain::TenantMember>, MembersError> {
        Err(MembersError::Unavailable("not used here".into()))
    }

    async fn add_member(
        &self,
        _: &ActiveSession,
        _: &Email,
        _: Role,
        _: CorrelationId,
    ) -> Result<identity_domain::TenantMember, MembersError> {
        Err(MembersError::Unavailable("not used here".into()))
    }

    async fn suspend_member(
        &self,
        _: &ActiveSession,
        _: MembershipId,
        _: CorrelationId,
    ) -> Result<(), MembersError> {
        Err(MembersError::Unavailable("not used here".into()))
    }
}

/// What the route asked for, and the answer it is scripted to get.
#[derive(Clone, Default)]
struct ScriptedDeactivations {
    answer: Option<MembersError>,
    calls: Arc<Mutex<Vec<(&'static str, UserId)>>>,
}

impl ScriptedDeactivations {
    fn refusing(answer: MembersError) -> Self {
        Self {
            answer: Some(answer),
            ..Self::default()
        }
    }

    fn calls(&self) -> Vec<(&'static str, UserId)> {
        self.calls
            .lock()
            .map(|calls| calls.clone())
            .unwrap_or_default()
    }

    fn record(&self, what: &'static str, who: UserId) -> Result<(), MembersError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push((what, who));
        }
        match &self.answer {
            Some(err) => Err(err.clone()),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl Deactivations for ScriptedDeactivations {
    async fn deactivate_member(
        &self,
        session: &ActiveSession,
        _: MembershipId,
        _: CorrelationId,
    ) -> Result<(), MembersError> {
        self.record("deactivate_member", session.user_id)
    }

    async fn reactivate_member(
        &self,
        session: &ActiveSession,
        _: MembershipId,
        _: CorrelationId,
    ) -> Result<(), MembersError> {
        self.record("reactivate_member", session.user_id)
    }

    async fn deactivate_self(
        &self,
        session: &ActiveSession,
        _: CorrelationId,
    ) -> Result<(), MembersError> {
        self.record("deactivate_self", session.user_id)
    }
}

struct World {
    app: Router,
    deactivations: ScriptedDeactivations,
}

fn world_with(deactivations: ScriptedDeactivations) -> World {
    let app = identity_router(IdentityState::new(
        Arc::new(RoleAuth),
        Arc::new(NoResets),
        Arc::new(NoMembers),
        Arc::new(deactivations.clone()),
        Arc::new(InMemoryRateLimiter::new(SystemClock)),
        reset_queue(1).0,
    ));
    World { app, deactivations }
}

fn world() -> World {
    world_with(ScriptedDeactivations::default())
}

fn post(uri: &str, caller: Option<&str>) -> Result<Request<Body>, Box<dyn Error>> {
    let mut builder = Request::builder().method("POST").uri(uri);
    if let Some(caller) = caller {
        builder = builder.header(header::COOKIE, format!("__Host-session={caller}"));
    }
    Ok(builder.body(Body::empty())?)
}

fn deactivate_member_uri() -> String {
    format!("/tenant/members/{}/deactivate", membership().as_uuid())
}

fn reactivate_member_uri() -> String {
    format!("/tenant/members/{}/reactivate", membership().as_uuid())
}

/// An Owner deactivating a member gets `204` and no body, and the use case
/// was actually reached.
#[tokio::test]
async fn an_owner_deactivates_a_member() -> Result<(), Box<dyn Error>> {
    let w = world();

    let response = w
        .app
        .clone()
        .oneshot(post(&deactivate_member_uri(), Some("owner"))?)
        .await?;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        w.deactivations.calls(),
        vec![("deactivate_member", caller())]
    );
    Ok(())
}

/// Reactivation is the symmetric route, and reaches its own use case.
#[tokio::test]
async fn an_admin_reactivates_a_member() -> Result<(), Box<dyn Error>> {
    let w = world();

    let response = w
        .app
        .clone()
        .oneshot(post(&reactivate_member_uri(), Some("admin"))?)
        .await?;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        w.deactivations.calls(),
        vec![("reactivate_member", caller())]
    );
    Ok(())
}

/// **The refusal says nothing about why.** The service answers `Forbidden`
/// both for "you may not" and for "they belong to another tenant"; this
/// pins that the wire keeps them identical -- same status, same body -- so
/// an Admin cannot learn from a status code that the target belongs to a
/// tenant they have no part in.
#[tokio::test]
async fn every_refusal_is_the_same_403() -> Result<(), Box<dyn Error>> {
    let mut answers = Vec::new();
    for caller in ["owner", "member"] {
        let w = world_with(ScriptedDeactivations::refusing(MembersError::Forbidden));
        let response = w
            .app
            .clone()
            .oneshot(post(&deactivate_member_uri(), Some(caller))?)
            .await?;
        let status = response.status();
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await?)?;
        answers.push((status, body));
    }

    assert_eq!(answers[0].0, StatusCode::FORBIDDEN);
    // Identical but for the correlation id, which is per-request by design.
    let normalise = |mut body: Value| {
        if let Some(object) = body.as_object_mut() {
            object.remove("correlation_id");
        }
        body
    };
    assert_eq!(
        normalise(answers[0].1.clone()),
        normalise(answers[1].1.clone())
    );
    Ok(())
}

/// A membership that is not this tenant's is `404`, the members routes'
/// existing contract for an id it may not see.
#[tokio::test]
async fn another_tenants_membership_is_404() -> Result<(), Box<dyn Error>> {
    let w = world_with(ScriptedDeactivations::refusing(MembersError::NotFound));

    let response = w
        .app
        .clone()
        .oneshot(post(&deactivate_member_uri(), Some("owner"))?)
        .await?;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    Ok(())
}

/// No cookie is `401`, not `403`: the caller is not authenticated at all,
/// and the frontend reads `401` as "log in again".
#[tokio::test]
async fn an_anonymous_caller_is_401_on_every_route() -> Result<(), Box<dyn Error>> {
    let w = world();

    for uri in [
        deactivate_member_uri(),
        reactivate_member_uri(),
        "/auth/deactivate".to_string(),
    ] {
        let response = w.app.clone().oneshot(post(&uri, None)?).await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
    assert!(w.deactivations.calls().is_empty());
    Ok(())
}

/// Closing your own account clears the session cookie in the same response:
/// every session is gone, so the cookie left in the browser could only
/// produce a `401` on the next request.
#[tokio::test]
async fn deactivating_yourself_clears_the_session_cookie() -> Result<(), Box<dyn Error>> {
    let w = world();

    let response = w
        .app
        .clone()
        .oneshot(post("/auth/deactivate", Some("member"))?)
        .await?;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(cookie.starts_with("__Host-session=;"), "{cookie}");
    assert!(cookie.contains("Max-Age=0"), "{cookie}");
    assert_eq!(w.deactivations.calls(), vec![("deactivate_self", caller())]);
    Ok(())
}

/// Self-deactivation needs no tenant picked: a person may leave without
/// first choosing which tenant to leave from.
#[tokio::test]
async fn deactivating_yourself_needs_no_tenant_picked() -> Result<(), Box<dyn Error>> {
    let w = world();

    let response = w
        .app
        .clone()
        .oneshot(post("/auth/deactivate", Some("unscoped"))?)
        .await?;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(w.deactivations.calls(), vec![("deactivate_self", caller())]);
    Ok(())
}
