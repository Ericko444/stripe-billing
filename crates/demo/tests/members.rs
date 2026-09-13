//! Inviting a member, end to end: the identity router the demo serves, the
//! real services, Argon2id, and Postgres. An Owner adds an address; the
//! invitee cannot log in until the mailed link sets a password, then can.

use std::error::Error;
use std::future::{Future, ready};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
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
    let app = identity_router(IdentityState::new(
        Arc::new(authentication),
        Arc::new(resets),
        Arc::new(members),
        Arc::new(InMemoryRateLimiter::new(SystemClock)),
        reset_queue(1).0,
    ));

    Ok(World {
        pool,
        app,
        outbox,
        _container: container,
    })
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
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string);
    Ok((response.status(), cookie))
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
