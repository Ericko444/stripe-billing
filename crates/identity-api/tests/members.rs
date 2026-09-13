//! `GET` and `POST /tenant/members` and `POST /tenant/members/{id}/suspend`
//! through the real router and the real `MembersService`, over a scripted
//! repository: who may do what, the one body for a new and an existing
//! address, and the one 404 for a membership that is not this tenant's.

use std::error::Error;
use std::future::{Future, ready};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use audit::CorrelationId;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use identity_api::{IdentityState, identity_router};
use identity_domain::{
    Email, GrantOutcome, MailError, MailPurpose, Mailer, MemberGrant, MemberRepository,
    MemberSuspension, MembershipId, MembershipStatus, OutgoingMail, Password, RepositoryError,
    Role, SessionId, SessionTenant, TenantId, TenantMember, UserId,
};
use identity_service::{
    ActiveSession, Authentication, CompleteResetError, InMemoryRateLimiter, LoginError,
    LoginOutcome, Me, MembersService, PasswordChangeError, PasswordResets, ProfileError,
    SelectTenantError, SessionError, SystemClock, TenantSelection, reset_queue,
};
use serde_json::Value;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

fn tenant_a() -> TenantId {
    TenantId::new(Uuid::from_u128(0xa))
}

/// The cookie value is the caller: `owner`, `admin` and `member` are
/// sessions in Tenant A with that role, `unscoped` has no tenant picked, and
/// anything else is refused.
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
        Err(LoginError::Unavailable("not used".into()))
    }

    async fn me(&self, _: &ActiveSession) -> Result<Me, SessionError> {
        Err(SessionError::Unavailable("not used".into()))
    }

    async fn select_tenant(
        &self,
        _: &ActiveSession,
        _: TenantId,
        _: CorrelationId,
    ) -> Result<TenantSelection, SelectTenantError> {
        Err(SelectTenantError::Unavailable("not used".into()))
    }

    async fn logout(&self, _: &ActiveSession) -> Result<(), SessionError> {
        Err(SessionError::Unavailable("not used".into()))
    }

    async fn update_display_name(
        &self,
        _: &ActiveSession,
        _: &str,
        _: CorrelationId,
    ) -> Result<Me, ProfileError> {
        Err(ProfileError::Unavailable("not used".into()))
    }

    async fn change_password(
        &self,
        _: &ActiveSession,
        _: &Password,
        _: Password,
        _: CorrelationId,
    ) -> Result<(), PasswordChangeError> {
        Err(PasswordChangeError::Unavailable("not used".into()))
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
        Err(CompleteResetError::Unavailable("not used".into()))
    }
}

/// `bob@example.test` has an account with a password; `taken@example.test`
/// is already in the tenant; any other address is new. For suspension, Tenant
/// A holds `OWNER_MEMBERSHIP` (an Owner), `MEMBER_MEMBERSHIP` (a Member) and
/// `CALLER_MEMBERSHIP` (the caller's own); every other id is not found.
#[derive(Clone, Default)]
struct ScriptedMembers {
    grants: Arc<Mutex<Vec<MemberGrant>>>,
    suspensions: Arc<Mutex<Vec<MemberSuspension>>>,
}

const OWNER_MEMBERSHIP: u128 = 0x0e;
const MEMBER_MEMBERSHIP: u128 = 0x0f;
const CALLER_MEMBERSHIP: u128 = 0x10;

/// The caller every `RoleAuth` session belongs to.
fn caller() -> UserId {
    UserId::new(Uuid::from_u128(1))
}

impl ScriptedMembers {
    fn grant_count(&self) -> usize {
        self.grants.lock().map(|g| g.len()).unwrap_or_default()
    }

    fn suspended(&self) -> Vec<MembershipId> {
        self.suspensions
            .lock()
            .map(|s| s.iter().map(|s| s.membership_id).collect())
            .unwrap_or_default()
    }
}

impl MemberRepository for ScriptedMembers {
    fn list(
        &self,
        tenant_id: TenantId,
    ) -> impl Future<Output = Result<Vec<TenantMember>, RepositoryError>> + Send {
        let listed = Email::parse("taken@example.test")
            .map(|email| {
                vec![TenantMember {
                    membership_id: MembershipId::new(Uuid::from_u128(
                        tenant_id.as_uuid().as_u128(),
                    )),
                    user_id: UserId::new(Uuid::from_u128(7)),
                    email,
                    role: Role::Member,
                    status: MembershipStatus::Suspended,
                }]
            })
            .map_err(|err| RepositoryError(err.to_string()));
        ready(listed)
    }

    fn grant(
        &self,
        grant: &MemberGrant,
    ) -> impl Future<Output = Result<GrantOutcome, RepositoryError>> + Send {
        if let Ok(mut grants) = self.grants.lock() {
            grants.push(grant.clone());
        }
        let outcome = match grant.email.as_str() {
            "taken@example.test" => GrantOutcome::AlreadyMember,
            address => GrantOutcome::Granted {
                member: TenantMember {
                    membership_id: MembershipId::new(Uuid::new_v4()),
                    user_id: UserId::new(Uuid::new_v4()),
                    email: grant.email.clone(),
                    role: grant.role,
                    status: MembershipStatus::Active,
                },
                invitation_issued: address != "bob@example.test",
            },
        };
        ready(Ok(outcome))
    }

    fn find(
        &self,
        tenant_id: TenantId,
        membership_id: MembershipId,
    ) -> impl Future<Output = Result<Option<TenantMember>, RepositoryError>> + Send {
        let (user_id, role) = match membership_id.as_uuid().as_u128() {
            OWNER_MEMBERSHIP => (UserId::new(Uuid::from_u128(2)), Role::Owner),
            MEMBER_MEMBERSHIP => (UserId::new(Uuid::from_u128(3)), Role::Member),
            CALLER_MEMBERSHIP => (caller(), Role::Admin),
            _ => return ready(Ok(None)),
        };
        if tenant_id != tenant_a() {
            return ready(Ok(None));
        }
        let found = Email::parse("someone@example.test")
            .map(|email| {
                Some(TenantMember {
                    membership_id,
                    user_id,
                    email,
                    role,
                    status: MembershipStatus::Active,
                })
            })
            .map_err(|err| RepositoryError(err.to_string()));
        ready(found)
    }

    fn suspend(
        &self,
        suspension: &MemberSuspension,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send {
        if let Ok(mut suspensions) = self.suspensions.lock() {
            suspensions.push(suspension.clone());
        }
        ready(Ok(true))
    }
}

#[derive(Clone, Default)]
struct RecordingMailer {
    purposes: Arc<Mutex<Vec<MailPurpose>>>,
}

impl Mailer for RecordingMailer {
    fn send(&self, mail: &OutgoingMail) -> impl Future<Output = Result<(), MailError>> + Send {
        if let Ok(mut purposes) = self.purposes.lock() {
            purposes.push(mail.purpose);
        }
        ready(Ok(()))
    }
}

struct World {
    app: Router,
    members: ScriptedMembers,
    mailer: RecordingMailer,
}

fn world() -> World {
    let members = ScriptedMembers::default();
    let mailer = RecordingMailer::default();
    let service = MembersService::new(
        members.clone(),
        mailer.clone(),
        SystemClock,
        "http://localhost:5173",
    );
    let app = identity_router(IdentityState::new(
        Arc::new(RoleAuth),
        Arc::new(NoResets),
        Arc::new(service),
        Arc::new(InMemoryRateLimiter::new(SystemClock)),
        reset_queue(1).0,
    ));
    World {
        app,
        members,
        mailer,
    }
}

fn add(caller: &str, email: &str, role: &str) -> Result<Request<Body>, Box<dyn Error>> {
    Ok(Request::builder()
        .method("POST")
        .uri("/tenant/members")
        .header(header::COOKIE, format!("__Host-session={caller}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "email": email, "role": role }).to_string(),
        ))?)
}

fn list(caller: Option<&str>) -> Result<Request<Body>, Box<dyn Error>> {
    let mut builder = Request::builder().uri("/tenant/members");
    if let Some(caller) = caller {
        builder = builder.header(header::COOKIE, format!("__Host-session={caller}"));
    }
    Ok(builder.body(Body::empty())?)
}

async fn send(app: &Router, request: Request<Body>) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)?
    };
    Ok((status, body))
}

#[tokio::test]
async fn members_and_admins_granting_owner_are_forbidden() -> Result<(), Box<dyn Error>> {
    let w = world();

    for (caller, role) in [
        ("member", "member"),
        ("unscoped", "member"),
        ("admin", "owner"),
    ] {
        let (status, body) = send(&w.app, add(caller, "carol@example.test", role)?).await?;
        assert_eq!(status, StatusCode::FORBIDDEN, "{caller} granting {role}");
        assert_eq!(body["status"], 403);
    }
    for caller in ["member", "unscoped"] {
        assert_eq!(
            send(&w.app, list(Some(caller))?).await?.0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(w.members.grant_count(), 0);

    assert_eq!(
        send(&w.app, add("admin", "carol@example.test", "admin")?)
            .await?
            .0,
        StatusCode::CREATED
    );
    assert_eq!(
        send(&w.app, add("owner", "dave@example.test", "owner")?)
            .await?
            .0,
        StatusCode::CREATED
    );
    Ok(())
}

#[tokio::test]
async fn a_new_and_an_existing_address_get_the_same_body_apart_from_ids()
-> Result<(), Box<dyn Error>> {
    let w = world();

    let (new_status, mut new) = send(&w.app, add("owner", "carol@example.test", "member")?).await?;
    let (existing_status, mut existing) =
        send(&w.app, add("owner", "bob@example.test", "member")?).await?;

    assert_eq!(new_status, StatusCode::CREATED);
    assert_eq!(existing_status, StatusCode::CREATED);
    for body in [&mut new, &mut existing] {
        let object = body.as_object_mut().ok_or("body is not an object")?;
        assert!(
            object
                .remove("membership_id")
                .is_some_and(|id| id.is_string())
        );
        assert!(object.remove("user_id").is_some_and(|id| id.is_string()));
        object.remove("email");
    }
    assert_eq!(new, existing);
    assert_eq!(
        new,
        serde_json::json!({ "role": "member", "status": "active" })
    );

    // Only the mail -- which the tenant never sees -- differs.
    let purposes = w
        .mailer
        .purposes
        .lock()
        .map(|p| p.clone())
        .unwrap_or_default();
    assert_eq!(
        purposes,
        vec![MailPurpose::Invitation, MailPurpose::AddedToTenant]
    );
    Ok(())
}

#[tokio::test]
async fn an_existing_member_conflicts_and_bad_input_is_refused_before_the_service()
-> Result<(), Box<dyn Error>> {
    let w = world();

    let (status, body) = send(&w.app, add("owner", "taken@example.test", "member")?).await?;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["title"], "Already a member");
    let grants_so_far = w.members.grant_count();

    for (email, role) in [
        ("not an address", "member"),
        ("carol@example.test", "Owner"),
        ("carol@example.test", "superuser"),
    ] {
        let (status, _) = send(&w.app, add("owner", email, role)?).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{email} {role}");
    }
    assert_eq!(w.members.grant_count(), grants_so_far);
    Ok(())
}

#[tokio::test]
async fn listing_members_returns_items_and_needs_a_session() -> Result<(), Box<dyn Error>> {
    let w = world();

    let (status, body) = send(&w.app, list(Some("admin"))?).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"][0]["email"], "taken@example.test");
    assert_eq!(body["items"][0]["role"], "member");
    assert_eq!(body["items"][0]["status"], "suspended");
    assert_eq!(
        body["items"][0].as_object().map(|o| o.len()),
        Some(5),
        "no display name, nothing about a password"
    );

    assert_eq!(send(&w.app, list(None)?).await?.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        send(&w.app, list(Some("forged"))?).await?.0,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

fn suspend(caller: &str, membership: &str) -> Result<Request<Body>, Box<dyn Error>> {
    Ok(Request::builder()
        .method("POST")
        .uri(format!("/tenant/members/{membership}/suspend"))
        .header(header::COOKIE, format!("__Host-session={caller}"))
        .body(Body::empty())?)
}

fn id(raw: u128) -> String {
    Uuid::from_u128(raw).to_string()
}

#[tokio::test]
async fn suspending_is_204_and_refuses_self_admins_on_owners_and_members()
-> Result<(), Box<dyn Error>> {
    let w = world();

    for (caller, membership, expected) in [
        ("admin", id(CALLER_MEMBERSHIP), StatusCode::FORBIDDEN),
        ("admin", id(OWNER_MEMBERSHIP), StatusCode::FORBIDDEN),
        ("member", id(MEMBER_MEMBERSHIP), StatusCode::FORBIDDEN),
        ("unscoped", id(MEMBER_MEMBERSHIP), StatusCode::FORBIDDEN),
    ] {
        let (status, _) = send(&w.app, suspend(caller, &membership)?).await?;
        assert_eq!(status, expected, "{caller} suspending {membership}");
    }
    assert!(w.members.suspended().is_empty());

    let (status, body) = send(&w.app, suspend("admin", &id(MEMBER_MEMBERSHIP))?).await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);
    let (status, _) = send(&w.app, suspend("owner", &id(OWNER_MEMBERSHIP))?).await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        w.members.suspended(),
        vec![
            MembershipId::new(Uuid::from_u128(MEMBER_MEMBERSHIP)),
            MembershipId::new(Uuid::from_u128(OWNER_MEMBERSHIP)),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_membership_that_is_not_this_tenants_is_one_404_and_a_bad_id_is_400()
-> Result<(), Box<dyn Error>> {
    let w = world();

    let mut bodies = Vec::new();
    for unknown in [id(0x999), id(0x998)] {
        let (status, mut body) = send(&w.app, suspend("owner", &unknown)?).await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
        if let Some(object) = body.as_object_mut() {
            object.remove("correlation_id");
        }
        bodies.push(body);
    }
    assert_eq!(bodies[0], bodies[1]);

    let (status, body) = send(&w.app, suspend("owner", "not-a-uuid")?).await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["status"], 400);
    assert!(w.members.suspended().is_empty());
    Ok(())
}
