//! The boundary proof: billing's `TenantExtractor`, satisfied by a
//! session from the identity module, with no change to `crates/api`.
//!
//! Runs the real pieces end to end -- `identity-pg` on a real Postgres,
//! `AuthService` with the real Argon2id hasher and system clock,
//! `identity-api`'s extractor -- behind `IdentityTenant`, on a route whose
//! state is billing's `AppState`.

mod common;

use std::error::Error;
use std::sync::Arc;

use api::billing_router;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::Extension;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use common::unused_state;
use demo::identity_tenant::IdentityTenant;
use identity_api::IdentityState;
use identity_domain::{NewSession, SessionRepository, SplitToken, TenantId, UserId};
use identity_pg::{PgMembershipRepository, PgSessionRepository, PgUserRepository};
use identity_service::{Argon2Hasher, AuthService, InMemoryRateLimiter, SystemClock};
use secrecy::ExposeSecret;
use serde_json::Value;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use testcontainers::core::wait::LogWaitStrategy;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

/// Compiles only if `IdentityTenant` satisfies `TenantExtractor` -- which
/// is the claim. Never called: mounting billing's real routes would reach
/// `unused_state`'s doubles.
#[allow(dead_code)]
fn billing_router_accepts_identity_tenant() -> Router {
    billing_router::<IdentityTenant>(unused_state())
}

async fn whoami(tenant: IdentityTenant) -> impl IntoResponse {
    let tenant_id: domain::TenantId = tenant.into();
    tenant_id.as_uuid().to_string()
}

struct World {
    pool: PgPool,
    app: Router,
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

    let authentication = AuthService::new(
        PgUserRepository::new(pool.clone()),
        PgMembershipRepository::new(pool.clone()),
        PgSessionRepository::new(pool.clone()),
        Argon2Hasher::new()?,
        SystemClock,
    );

    // The shape `main` will build: a route whose state is billing's
    // `AppState`, behind `IdentityTenant`, with the identity state installed
    // as an extension over it.
    let app = Router::new()
        .route("/whoami", get(whoami))
        .with_state(unused_state())
        .layer(Extension(IdentityState::new(
            Arc::new(authentication),
            Arc::new(NoResets),
            Arc::new(NoMembers),
            Arc::new(InMemoryRateLimiter::new(SystemClock)),
            identity_service::reset_queue(1).0,
        )));

    Ok(World {
        pool,
        app,
        _container: container,
    })
}

async fn seed_user_in_tenant(
    pool: &PgPool,
    membership_status: &str,
) -> Result<(UserId, TenantId), Box<dyn Error>> {
    let user = Uuid::new_v4();
    let tenant = Uuid::new_v4();
    sqlx::query("INSERT INTO identity.tenants (id, name) VALUES ($1, 'Tenant A')")
        .bind(tenant)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO identity.users (id, email_normalized) VALUES ($1, $2)")
        .bind(user)
        .bind(format!("{user}@example.test"))
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO identity.memberships (id, user_id, tenant_id, role, status) \
         VALUES ($1, $2, $3, 'member', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(user)
    .bind(tenant)
    .bind(membership_status)
    .execute(pool)
    .await?;
    Ok((UserId::new(user), TenantId::new(tenant)))
}

/// Stores a session and returns the cookie a browser would send for it.
async fn session_cookie(
    pool: &PgPool,
    user: UserId,
    tenant: Option<TenantId>,
    expires_in: Duration,
) -> Result<String, Box<dyn Error>> {
    let token = identity_service::generate_token()?;
    let now = OffsetDateTime::now_utc();
    let expires_at = now + expires_in;
    PgSessionRepository::new(pool.clone())
        .create(
            &NewSession {
                selector: token.selector(),
                verifier_hash: token.verifier().hash(),
                user_id: user,
                tenant_id: tenant,
                authenticated_at: expires_at - Duration::hours(8),
                expires_at,
            },
            None,
        )
        .await?;
    Ok(cookie_for(&token))
}

fn cookie_for(token: &SplitToken) -> String {
    format!("__Host-session={}", token.to_wire().expose_secret())
}

async fn call(app: &Router, cookie: Option<&str>) -> Result<(StatusCode, Vec<u8>), Box<dyn Error>> {
    let mut request = Request::builder().uri("/whoami");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    let response = app.clone().oneshot(request.body(Body::empty())?).await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    Ok((status, bytes.to_vec()))
}

fn without_correlation_id(bytes: &[u8]) -> Result<Value, Box<dyn Error>> {
    let mut body: Value = serde_json::from_slice(bytes)?;
    if let Some(object) = body.as_object_mut() {
        object.remove("correlation_id");
    }
    Ok(body)
}

#[tokio::test]
async fn a_tenant_scoped_identity_session_reaches_a_billing_handler_as_its_tenant()
-> Result<(), Box<dyn Error>> {
    let w = world().await?;
    let (user, tenant) = seed_user_in_tenant(&w.pool, "active").await?;
    let cookie = session_cookie(&w.pool, user, Some(tenant), Duration::hours(1)).await?;

    let (status, body) = call(&w.app, Some(&cookie)).await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, tenant.as_uuid().to_string().as_bytes());
    Ok(())
}

#[tokio::test]
async fn every_refusal_is_billings_one_identical_401() -> Result<(), Box<dyn Error>> {
    let w = world().await?;

    let (user, tenant) = seed_user_in_tenant(&w.pool, "active").await?;
    let tenant_less = session_cookie(&w.pool, user, None, Duration::minutes(5)).await?;
    let expired = session_cookie(&w.pool, user, Some(tenant), Duration::seconds(-1)).await?;

    let (suspended_user, suspended_tenant) = seed_user_in_tenant(&w.pool, "suspended").await?;
    let suspended = session_cookie(
        &w.pool,
        suspended_user,
        Some(suspended_tenant),
        Duration::hours(1),
    )
    .await?;

    let mut bodies = Vec::new();
    for (case, cookie) in [
        ("no cookie", None),
        ("tenant-less session", Some(tenant_less.as_str())),
        ("expired session", Some(expired.as_str())),
        ("suspended membership", Some(suspended.as_str())),
        ("forged token", Some("__Host-session=00.00")),
    ] {
        let (status, body) = call(&w.app, cookie).await?;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{case}");
        bodies.push(without_correlation_id(&body)?);
    }

    assert!(
        bodies.windows(2).all(|pair| pair[0] == pair[1]),
        "{bodies:?}"
    );
    Ok(())
}

/// The boundary proof never completes a password reset.
struct NoResets;

#[async_trait::async_trait]
impl identity_service::PasswordResets for NoResets {
    async fn complete_reset(
        &self,
        _: &str,
        _: identity_domain::Password,
        _: audit::CorrelationId,
    ) -> Result<(), identity_service::CompleteResetError> {
        Err(identity_service::CompleteResetError::Unavailable(
            "not used by the boundary proof".into(),
        ))
    }
}

/// The boundary proof never manages members.
struct NoMembers;

#[async_trait::async_trait]
impl identity_service::Members for NoMembers {
    async fn list_members(
        &self,
        _: &identity_service::ActiveSession,
    ) -> Result<Vec<identity_domain::TenantMember>, identity_service::MembersError> {
        Err(identity_service::MembersError::Unavailable(
            "not used by the boundary proof".into(),
        ))
    }

    async fn add_member(
        &self,
        _: &identity_service::ActiveSession,
        _: &identity_domain::Email,
        _: identity_domain::Role,
        _: audit::CorrelationId,
    ) -> Result<identity_domain::TenantMember, identity_service::MembersError> {
        Err(identity_service::MembersError::Unavailable(
            "not used by the boundary proof".into(),
        ))
    }

    async fn suspend_member(
        &self,
        _: &identity_service::ActiveSession,
        _: identity_domain::MembershipId,
        _: audit::CorrelationId,
    ) -> Result<(), identity_service::MembersError> {
        Err(identity_service::MembersError::Unavailable(
            "not used by the boundary proof".into(),
        ))
    }
}
