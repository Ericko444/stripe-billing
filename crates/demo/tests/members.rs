//! Members, end to end: the identity router the demo serves, the real
//! services, Argon2id, and Postgres, beside a billing-side route behind
//! `IdentityTenant`. An Owner adds an address, and the invitee cannot log in
//! until the mailed link sets a password; an Owner suspends a member, and
//! the member's next billing request in that tenant is refused while their
//! session in another tenant goes on working.

mod common;

use std::error::Error;
use std::future::{Future, ready};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Extension};
use axum::http::{Request, Response, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use common::unused_state;
use demo::identity_tenant::IdentityTenant;
use identity_api::{IdentityState, identity_router};
use identity_domain::{
    MailError, MailPurpose, Mailer, NewPassword, OutgoingMail, Password, PasswordHasher,
};
use identity_pg::{
    PgMembershipRepository, PgPasswordTokenRepository, PgSessionRepository, PgUserRepository,
};
use identity_service::{
    Argon2Hasher, AuthService, InMemoryRateLimiter, MembersService, PasswordResetService,
    SystemClock, reset_queue,
};
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use testcontainers::core::wait::LogWaitStrategy;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tower::ServiceExt;
use uuid::Uuid;

const OWNER_PASSWORD: &str = "the owner's long passphrase";
const INVITEE_PASSWORD: &str = "carol chose this long passphrase";

/// Keeps every mail, link exposed, as the demo's `LogMailer` would log it.
#[derive(Clone, Default)]
struct Outbox(Arc<Mutex<Vec<(String, MailPurpose, String)>>>);

impl Outbox {
    fn last_to(&self, address: &str) -> Option<(MailPurpose, String)> {
        self.0.lock().ok().and_then(|mails| {
            mails
                .iter()
                .rev()
                .find(|(to, _, _)| to == address)
                .map(|(_, purpose, link)| (*purpose, link.clone()))
        })
    }
}

impl Mailer for Outbox {
    fn send(&self, mail: &OutgoingMail) -> impl Future<Output = Result<(), MailError>> + Send {
        if let Ok(mut mails) = self.0.lock() {
            mails.push((
                mail.to.as_str().to_string(),
                mail.purpose,
                mail.link.expose_secret().to_string(),
            ));
        }
        ready(Ok(()))
    }
}

struct World {
    pool: PgPool,
    app: Router,
    outbox: Outbox,
    _container: ContainerAsync<GenericImage>,
}

async fn world() -> Result<World, Box<dyn Error>> {
    let container = GenericImage::new("postgres", "16-alpine")
        .with_wait_for(WaitFor::log(
            LogWaitStrategy::stdout_or_stderr("database system is ready to accept connections")
                .with_times(2),
        ))
        .with_exposed_port(5432.tcp())
        .with_env_var("POSTGRES_PASSWORD", "postgres")
        .start()
        .await?;
    let port = container.get_host_port_ipv4(5432.tcp()).await?;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = PgPoolOptions::new().connect(&url).await?;
    identity_pg::run_migrations(&pool).await?;
    audit_pg::run_migrations(&pool).await?;

    // Wired as `main` wires it, with the outbox standing in for `LogMailer`.
    let hasher = Argon2Hasher::new()?;
    let outbox = Outbox::default();
    let authentication = AuthService::new(
        PgUserRepository::new(pool.clone()),
        PgMembershipRepository::new(pool.clone()),
        PgSessionRepository::new(pool.clone()),
        hasher.clone(),
        SystemClock,
    );
    let resets = PasswordResetService::new(
        PgUserRepository::new(pool.clone()),
        PgPasswordTokenRepository::new(pool.clone()),
        hasher,
        outbox.clone(),
        SystemClock,
        "http://localhost:5173",
    );
    let members = MembersService::new(
        PgMembershipRepository::new(pool.clone()),
        outbox.clone(),
        SystemClock,
        "http://localhost:5173",
    );
    let identity = IdentityState::new(
        Arc::new(authentication),
        Arc::new(resets),
        Arc::new(members),
        Arc::new(NoDeactivations),
        Arc::new(InMemoryRateLimiter::new(SystemClock)),
        reset_queue(1).0,
    );
    // A route in billing's state, behind billing's extractor, merged with
    // the identity routes under one `Extension` -- the shape `main` serves.
    let app = Router::new()
        .route("/whoami", get(whoami))
        .with_state(unused_state())
        .merge(identity_router(identity.clone()))
        .layer(Extension(identity));

    Ok(World {
        pool,
        app,
        outbox,
        _container: container,
    })
}

async fn whoami(tenant: IdentityTenant) -> impl IntoResponse {
    let tenant_id: domain::TenantId = tenant.into();
    tenant_id.as_uuid().to_string()
}

/// A tenant, and a user with a real Argon2id password as its only member.
async fn seed_owner(pool: &PgPool, tenant_name: &str, email: &str) -> Result<Uuid, Box<dyn Error>> {
    let tenant = Uuid::new_v4();
    let user = Uuid::new_v4();
    let hash = Argon2Hasher::new()?
        .hash(&NewPassword::check(Password::new(
            OWNER_PASSWORD.to_string(),
        ))?)
        .await?;
    sqlx::query("INSERT INTO identity.tenants (id, name) VALUES ($1, $2)")
        .bind(tenant)
        .bind(tenant_name)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO identity.users (id, email_normalized, password_hash) VALUES ($1, $2, $3)",
    )
    .bind(user)
    .bind(email)
    .bind(hash.as_str())
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO identity.memberships (id, user_id, tenant_id, role, status) \
         VALUES ($1, $2, $3, 'owner', 'active')",
    )
    .bind(Uuid::new_v4())
    .bind(user)
    .bind(tenant)
    .execute(pool)
    .await?;
    Ok(tenant)
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

fn post(uri: &str, body: &Value, cookie: Option<&str>) -> Result<Request<Body>, Box<dyn Error>> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let mut request = builder.body(Body::from(body.to_string()))?;
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([203, 0, 113, 9], 50_000))));
    Ok(request)
}

/// Logs in and returns the status and, on success, the session cookie.
async fn login(
    app: &Router,
    email: &str,
    password: &str,
) -> Result<(StatusCode, Option<String>), Box<dyn Error>> {
    let response = app
        .clone()
        .oneshot(post(
            "/auth/login",
            &json!({ "email": email, "password": password }),
            None,
        )?)
        .await?;
    Ok((response.status(), session_cookie_of(&response)))
}

/// The `name=value` part of a response's `Set-Cookie`, as a browser would
/// send it back.
fn session_cookie_of(response: &Response<Body>) -> Option<String> {
    response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string)
}

/// A session of `email` scoped to `tenant`: log in (several memberships, so
/// no tenant yet), then pick it.
async fn session_in(app: &Router, email: &str, tenant: Uuid) -> Result<String, Box<dyn Error>> {
    let unscoped = login(app, email, OWNER_PASSWORD)
        .await?
        .1
        .ok_or("no session cookie")?;
    let response = app
        .clone()
        .oneshot(post(
            "/auth/tenant",
            &json!({ "tenant_id": tenant }),
            Some(&unscoped),
        )?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    session_cookie_of(&response).ok_or_else(|| "no rotated cookie".into())
}

async fn whoami_as(app: &Router, cookie: &str) -> Result<(StatusCode, String), Box<dyn Error>> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/whoami")
                .header(header::COOKIE, cookie)
                .body(Body::empty())?,
        )
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    Ok((status, String::from_utf8_lossy(&bytes).into_owned()))
}

#[tokio::test]
async fn an_invited_user_cannot_log_in_until_the_invitation_link_sets_a_password()
-> Result<(), Box<dyn Error>> {
    let w = world().await?;
    let tenant_a = seed_owner(&w.pool, "Tenant A", "owner@example.test").await?;
    let (status, owner) = login(&w.app, "owner@example.test", OWNER_PASSWORD).await?;
    assert_eq!(status, StatusCode::OK);
    let owner = owner.ok_or("no session cookie")?;

    let (status, _) = send(
        &w.app,
        post(
            "/tenant/members",
            &json!({ "email": "carol@example.test", "role": "member" }),
            Some(&owner),
        )?,
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);

    // No password yet: nothing verifies, whatever is typed.
    for attempt in ["", INVITEE_PASSWORD] {
        assert_eq!(
            login(&w.app, "carol@example.test", attempt).await?.0,
            StatusCode::UNAUTHORIZED
        );
    }

    let (purpose, link) = w
        .outbox
        .last_to("carol@example.test")
        .ok_or("no invitation was mailed")?;
    assert_eq!(purpose, MailPurpose::Invitation);
    let token = link
        .strip_prefix("http://localhost:5173/reset-password#token=")
        .ok_or("the invitation link has the wrong shape")?;
    let (status, _) = send(
        &w.app,
        post(
            "/auth/password-reset/complete",
            &json!({ "token": token, "new_password": INVITEE_PASSWORD }),
            None,
        )?,
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, carol) = login(&w.app, "carol@example.test", INVITEE_PASSWORD).await?;
    assert_eq!(status, StatusCode::OK);
    assert!(carol.is_some());

    // The link is spent.
    let (status, _) = send(
        &w.app,
        post(
            "/auth/password-reset/complete",
            &json!({ "token": token, "new_password": "yet another long passphrase" }),
            None,
        )?,
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // By time, then by action: one request's rows share an instant, and
    // `audit_log` ids are random.
    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit.audit_log WHERE tenant_id = $1 ORDER BY occurred_at, action",
    )
    .bind(tenant_a)
    .fetch_all(&w.pool)
    .await?;
    assert_eq!(
        actions,
        vec![
            "session.started",
            "membership.granted",
            "user.created",
            "invitation.accepted",
            "session.started",
        ]
    );
    Ok(())
}

#[tokio::test]
async fn adding_a_new_or_an_existing_address_answers_the_same_apart_from_ids()
-> Result<(), Box<dyn Error>> {
    let w = world().await?;
    seed_owner(&w.pool, "Tenant A", "owner@example.test").await?;
    seed_owner(&w.pool, "Tenant B", "bob@example.test").await?;
    let owner = login(&w.app, "owner@example.test", OWNER_PASSWORD)
        .await?
        .1
        .ok_or("no session cookie")?;

    let mut bodies = Vec::new();
    for address in ["carol@example.test", "bob@example.test"] {
        let (status, mut body) = send(
            &w.app,
            post(
                "/tenant/members",
                &json!({ "email": address, "role": "admin" }),
                Some(&owner),
            )?,
        )
        .await?;
        assert_eq!(status, StatusCode::CREATED, "{address}");
        if let Some(object) = body.as_object_mut() {
            for varies in ["membership_id", "user_id", "email"] {
                assert!(object.remove(varies).is_some(), "{address}: {varies}");
            }
        }
        bodies.push(body);
    }

    assert_eq!(bodies[0], bodies[1]);
    assert_eq!(
        w.outbox
            .last_to("bob@example.test")
            .map(|(purpose, _)| purpose),
        Some(MailPurpose::AddedToTenant)
    );
    Ok(())
}

async fn membership_of(pool: &PgPool, email: &str, tenant: Uuid) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT m.id FROM identity.memberships m \
           JOIN identity.users u ON u.id = m.user_id \
          WHERE u.email_normalized = $1 AND m.tenant_id = $2",
    )
    .bind(email)
    .bind(tenant)
    .fetch_one(pool)
    .await
}

#[tokio::test]
async fn a_suspended_member_loses_that_tenant_and_keeps_the_other() -> Result<(), Box<dyn Error>> {
    let w = world().await?;
    let tenant_a = seed_owner(&w.pool, "Tenant A", "owner@example.test").await?;
    let tenant_b = seed_owner(&w.pool, "Tenant B", "bob@example.test").await?;
    let owner = login(&w.app, "owner@example.test", OWNER_PASSWORD)
        .await?
        .1
        .ok_or("no session cookie")?;
    let (status, added) = send(
        &w.app,
        post(
            "/tenant/members",
            &json!({ "email": "bob@example.test", "role": "member" }),
            Some(&owner),
        )?,
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);
    let bob_in_a = added["membership_id"]
        .as_str()
        .ok_or("no membership id")?
        .to_string();

    let bob_a = session_in(&w.app, "bob@example.test", tenant_a).await?;
    let bob_b = session_in(&w.app, "bob@example.test", tenant_b).await?;
    assert_eq!(
        whoami_as(&w.app, &bob_a).await?,
        (StatusCode::OK, tenant_a.to_string())
    );

    let suspend = |membership: &str| {
        post(
            &format!("/tenant/members/{membership}/suspend"),
            &Value::Null,
            Some(&owner),
        )
    };
    let (status, _) = send(&w.app, suspend(&bob_in_a)?).await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Refused in Tenant A on the very next request; untouched in Tenant B.
    assert_eq!(whoami_as(&w.app, &bob_a).await?.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        whoami_as(&w.app, &bob_b).await?,
        (StatusCode::OK, tenant_b.to_string())
    );
    let sessions_left_in_a: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM identity.sessions s \
           JOIN identity.users u ON u.id = s.user_id \
          WHERE u.email_normalized = 'bob@example.test' AND s.tenant_id = $1",
    )
    .bind(tenant_a)
    .fetch_one(&w.pool)
    .await?;
    assert_eq!(sessions_left_in_a, 0);

    // Bob's membership of Tenant B, named from Tenant A, is the same 404 as
    // an id that does not exist.
    let bob_in_b = membership_of(&w.pool, "bob@example.test", tenant_b).await?;
    let mut bodies = Vec::new();
    for membership in [bob_in_b, Uuid::new_v4()] {
        let (status, mut body) = send(&w.app, suspend(&membership.to_string())?).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{membership}");
        if let Some(object) = body.as_object_mut() {
            object.remove("correlation_id");
        }
        bodies.push(body);
    }
    assert_eq!(bodies[0], bodies[1]);
    assert_eq!(
        whoami_as(&w.app, &bob_b).await?.0,
        StatusCode::OK,
        "a 404 changed nothing"
    );

    // And nobody suspends themselves.
    let own = membership_of(&w.pool, "owner@example.test", tenant_a).await?;
    assert_eq!(
        send(&w.app, suspend(&own.to_string())?).await?.0,
        StatusCode::FORBIDDEN
    );

    let suspended: Vec<(String, Option<Uuid>)> = sqlx::query_as(
        "SELECT tenant_id::text, target_id FROM audit.audit_log \
          WHERE action = 'membership.suspended'",
    )
    .fetch_all(&w.pool)
    .await?;
    assert_eq!(
        suspended,
        vec![(tenant_a.to_string(), Some(Uuid::parse_str(&bob_in_a)?))]
    );
    Ok(())
}

/// Deactivation is not exercised by these tests; every call is an error
/// rather than a silent success, so a route that reached it would fail
/// loudly.
struct NoDeactivations;

#[async_trait::async_trait]
impl identity_service::Deactivations for NoDeactivations {
    async fn deactivate_member(
        &self,
        _: &identity_service::ActiveSession,
        _: identity_domain::MembershipId,
        _: audit::CorrelationId,
    ) -> Result<(), identity_service::MembersError> {
        Err(identity_service::MembersError::Unavailable(
            "not used by these tests".into(),
        ))
    }

    async fn reactivate_member(
        &self,
        _: &identity_service::ActiveSession,
        _: identity_domain::MembershipId,
        _: audit::CorrelationId,
    ) -> Result<(), identity_service::MembersError> {
        Err(identity_service::MembersError::Unavailable(
            "not used by these tests".into(),
        ))
    }

    async fn deactivate_self(
        &self,
        _: &identity_service::ActiveSession,
        _: audit::CorrelationId,
    ) -> Result<(), identity_service::MembersError> {
        Err(identity_service::MembersError::Unavailable(
            "not used by these tests".into(),
        ))
    }
}
