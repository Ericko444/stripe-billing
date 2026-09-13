use audit::CorrelationId;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use identity_domain::{Email, MembershipId, Role};
use identity_service::MembersError;
use uuid::Uuid;

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
