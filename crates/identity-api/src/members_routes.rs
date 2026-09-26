use audit::CorrelationId;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Extension, Path};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use identity_domain::{Email, MembershipId, Role};
use identity_service::MembersError;
use uuid::Uuid;

use crate::cookie;
use crate::dto::{AddMemberRequest, MemberDto, MembersDto};
use crate::error::{ErrorKind, IdentityError};
use crate::session_extract::AuthenticatedSession;
use crate::state::IdentityState;

fn members_error(err: MembersError, correlation_id: CorrelationId) -> IdentityError {
    let kind = match err {
        MembersError::Forbidden => ErrorKind::Forbidden,
        MembersError::NotFound => ErrorKind::NotFound,
        MembersError::AlreadyMember => ErrorKind::AlreadyMember,
        MembersError::Unavailable(reason) => ErrorKind::Internal(reason),
    };
    IdentityError::new(kind, correlation_id)
}

/// `GET /tenant/members`: every membership of the session's tenant, for its
/// Owners and Admins. `403` for anyone else, including a session with no
/// tenant picked -- the caller is authenticated, so `401` would be wrong and
/// would log them out.
pub async fn list_members(
    Extension(state): Extension<IdentityState>,
    Extension(correlation_id): Extension<CorrelationId>,
    session: AuthenticatedSession,
) -> Result<Response, IdentityError> {
    let members = state
        .members()
        .list_members(session.session())
        .await
        .map_err(|err| members_error(err, correlation_id))?;

    Ok(Json(MembersDto {
        items: members.iter().map(MemberDto::from).collect(),
    })
    .into_response())
}

/// `POST /tenant/members`: add an address to the session's tenant.
///
/// `201` with the membership -- **the same body whether or not the address
/// already had an account**, apart from the ids. `403` for a caller who may
/// not grant that role, `409` if the address is already a member here, `400`
/// for an address or role that does not parse.
pub async fn add_member(
    Extension(state): Extension<IdentityState>,
    Extension(correlation_id): Extension<CorrelationId>,
    session: AuthenticatedSession,
    body: Result<Json<AddMemberRequest>, JsonRejection>,
) -> Result<Response, IdentityError> {
    let error = |kind| IdentityError::new(kind, correlation_id);
    let Json(request) =
        body.map_err(|rejection| error(ErrorKind::MalformedRequest(rejection.body_text())))?;
    let email = Email::parse(&request.email)
        .map_err(|_| error(ErrorKind::MalformedRequest("invalid email address".into())))?;
    let role = request
        .role
        .parse::<Role>()
        .map_err(|_| error(ErrorKind::MalformedRequest("unknown role".into())))?;

    let member = state
        .members()
        .add_member(session.session(), &email, role, correlation_id)
        .await
        .map_err(|err| members_error(err, correlation_id))?;

    Ok((StatusCode::CREATED, Json(MemberDto::from(&member))).into_response())
}

/// `POST /tenant/members/{id}/suspend`: suspend a membership of the session's
/// tenant.
///
/// `204` on success -- also for a membership already suspended. `404` for an
/// id that is not a membership **of this tenant**, with the same body whether
/// it is unknown or another tenant's; `403` for suspending oneself, an Admin
/// suspending an Owner, or a caller who manages no members here.
pub async fn suspend_member(
    Extension(state): Extension<IdentityState>,
    Extension(correlation_id): Extension<CorrelationId>,
    session: AuthenticatedSession,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Response, IdentityError> {
    let Path(id) = id.map_err(|rejection| {
        IdentityError::new(
            ErrorKind::MalformedRequest(rejection.body_text()),
            correlation_id,
        )
    })?;

    state
        .members()
        .suspend_member(session.session(), MembershipId::new(id), correlation_id)
        .await
        .map_err(|err| members_error(err, correlation_id))?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /tenant/members/{id}/deactivate`: close the account behind a
/// membership of the session's tenant, in **every** tenant it belongs to.
///
/// `204` on success -- also for an account already deactivated, which is the
/// state that was asked for. `404` for an id that is not a membership of this
/// tenant. `403` for deactivating oneself (use `POST /auth/deactivate`), an
/// Admin reaching an Owner, a caller who manages no members here, **and for
/// an account that also belongs to another tenant**.
///
/// That last case deliberately shares its answer with the others. A distinct
/// status or message would tell an Admin of one tenant that the target also
/// belongs to a tenant they have no part in -- a membership they cannot
/// otherwise see. The tenant-scoped instrument for "not in my tenant any
/// more" is `POST /tenant/members/{id}/suspend`, which is always available.
pub async fn deactivate_member(
    Extension(state): Extension<IdentityState>,
    Extension(correlation_id): Extension<CorrelationId>,
    session: AuthenticatedSession,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Response, IdentityError> {
    let Path(id) = id.map_err(|rejection| {
        IdentityError::new(
            ErrorKind::MalformedRequest(rejection.body_text()),
            correlation_id,
        )
    })?;

    state
        .deactivations()
        .deactivate_member(session.session(), MembershipId::new(id), correlation_id)
        .await
        .map_err(|err| members_error(err, correlation_id))?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /tenant/members/{id}/reactivate`: restore a deactivated account
/// behind a membership of the session's tenant.
///
/// `204` on success -- also for an account that was not deactivated. `404`
/// and `403` as for deactivation, minus the sole-tenant case: reactivating
/// grants no access the account did not already have, so it cannot reach
/// into another tenant and is not refused for belonging to one.
///
/// There is no self-service counterpart, and cannot be: a deactivated user
/// cannot log in, so there is no session from which to ask.
pub async fn reactivate_member(
    Extension(state): Extension<IdentityState>,
    Extension(correlation_id): Extension<CorrelationId>,
    session: AuthenticatedSession,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Response, IdentityError> {
    let Path(id) = id.map_err(|rejection| {
        IdentityError::new(
            ErrorKind::MalformedRequest(rejection.body_text()),
            correlation_id,
        )
    })?;

    state
        .deactivations()
        .reactivate_member(session.session(), MembershipId::new(id), correlation_id)
        .await
        .map_err(|err| members_error(err, correlation_id))?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /auth/deactivate`: close the caller's own account.
///
/// `204`, with the session cookie cleared in the same response: every session
/// of the account is deleted, this one included, so leaving the cookie in the
/// browser would only produce a `401` on the next request. No tenant needs to
/// be picked -- a person may leave without first choosing which tenant to
/// leave from -- and no sole-tenant constraint applies: there is no other
/// tenant's interest to protect from someone closing their own account.
pub async fn deactivate_self(
    Extension(state): Extension<IdentityState>,
    Extension(correlation_id): Extension<CorrelationId>,
    session: AuthenticatedSession,
) -> Result<Response, IdentityError> {
    state
        .deactivations()
        .deactivate_self(session.session(), correlation_id)
        .await
        .map_err(|err| members_error(err, correlation_id))?;

    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie::clearing_cookie());
    Ok(response)
}
