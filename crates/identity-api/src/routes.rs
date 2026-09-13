use audit::CorrelationId;
use axum::Json;
use axum::extract::Extension;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use identity_domain::{Email, Password};
use identity_service::{
    LoginError, SessionError, SessionScope, TENANT_SESSION_LIFETIME, UNSCOPED_SESSION_LIFETIME,
};

use crate::cookie::session_cookie;
use crate::dto::{CurrentTenantDto, LoginRequest, MeDto, MembershipDto, SessionDto, rfc3339_utc};
use crate::error::{ErrorKind, IdentityError};
use crate::session_extract::AuthenticatedSession;
use crate::state::IdentityState;

/// `POST /auth/login`.
///
/// An address that does not parse is answered exactly like a wrong password:
/// the caller learns nothing more from a typo than from a guess.
pub async fn login(
    Extension(state): Extension<IdentityState>,
    Extension(correlation_id): Extension<CorrelationId>,
    body: Result<Json<LoginRequest>, JsonRejection>,
) -> Result<Response, IdentityError> {
    let error = |kind| IdentityError::new(kind, correlation_id);
    let Json(request) =
        body.map_err(|rejection| error(ErrorKind::MalformedRequest(rejection.body_text())))?;
    let password = Password::new(request.password);
    let email = Email::parse(&request.email).map_err(|_| error(ErrorKind::Unauthorized))?;

    let outcome = state
        .authentication()
        .login(&email, &password, correlation_id)
        .await
        .map_err(|err| match err {
            LoginError::InvalidCredentials => error(ErrorKind::Unauthorized),
            LoginError::Unavailable(reason) => error(ErrorKind::Internal(reason)),
        })?;

    let lifetime = match outcome.scope {
        SessionScope::Tenant(_) => TENANT_SESSION_LIFETIME,
        SessionScope::Unscoped => UNSCOPED_SESSION_LIFETIME,
    };
    let cookie = session_cookie(&outcome.token.to_wire(), lifetime).ok_or_else(|| {
        error(ErrorKind::Internal(
            "session token is not a valid header".into(),
        ))
    })?;
    let tenant = match &outcome.scope {
        SessionScope::Tenant(membership) => Some(CurrentTenantDto {
            tenant_id: membership.tenant_id.as_uuid(),
            tenant_name: membership.tenant_name.clone(),
            role: membership.role.as_str(),
        }),
        SessionScope::Unscoped => None,
    };
    let body = SessionDto {
        user_id: outcome.user_id.as_uuid(),
        tenant,
        memberships: outcome
            .memberships
            .iter()
            .map(MembershipDto::from)
            .collect(),
        expires_at: rfc3339_utc(outcome.expires_at),
    };

    // The token goes in `Set-Cookie` and nowhere else -- never in the body,
    // where script could read it.
    let mut response = Json(body).into_response();
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    no_store(&mut response);
    Ok(response)
}

/// `GET /auth/me`.
pub async fn me(
    Extension(state): Extension<IdentityState>,
    Extension(correlation_id): Extension<CorrelationId>,
    session: AuthenticatedSession,
) -> Result<Response, IdentityError> {
    let me = state
        .authentication()
        .me(session.session())
        .await
        .map_err(|err| match err {
            SessionError::Unauthenticated => {
                IdentityError::new(ErrorKind::Unauthorized, correlation_id)
            }
            SessionError::Unavailable(reason) => {
                IdentityError::new(ErrorKind::Internal(reason), correlation_id)
            }
        })?;

    let mut response = Json(MeDto {
        user_id: me.user_id.as_uuid(),
        email: me.email.as_str().to_string(),
        display_name: me.display_name,
        tenant: CurrentTenantDto::find(me.tenant, &me.memberships),
        memberships: me.memberships.iter().map(MembershipDto::from).collect(),
        expires_at: rfc3339_utc(me.expires_at),
    })
    .into_response();
    no_store(&mut response);
    Ok(response)
}

/// Responses about a session are never cached -- by the browser, or by any
/// proxy between it and us.
fn no_store(response: &mut Response) {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}
