//! Wire types. Owned by this crate and built by hand from service types --
//! the billing module's rule: nothing in `identity-domain` or
//! `identity-service` is `Serialize`, so a field added there cannot silently
//! become a field on the wire.

use identity_domain::{Membership, SessionTenant};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, UtcOffset};
use uuid::Uuid;

/// `POST /auth/login` body.
///
/// Deliberately no `Debug`: a derived one would print the password. The
/// handler moves `password` into a `Password` secret immediately.
#[derive(Deserialize)]
pub struct LoginRequest {
    /// The address, as typed.
    pub email: String,
    /// The password, as typed.
    pub password: String,
}

/// `POST /auth/tenant` body. The tenant is chosen by the caller -- and then
/// checked against the caller's own active memberships, which is what makes
/// accepting it from a body safe here when billing accepts a tenant from
/// nowhere but the session.
#[derive(Debug, Deserialize)]
pub struct SelectTenantRequest {
    /// The tenant to scope the session to.
    pub tenant_id: Uuid,
}

/// A membership, for a tenant picker.
#[derive(Debug, Serialize)]
pub struct MembershipDto {
    /// The membership's id.
    pub membership_id: Uuid,
    /// The tenant's id.
    pub tenant_id: Uuid,
    /// The tenant's display name.
    pub tenant_name: String,
    /// `owner`, `admin` or `member`.
    pub role: &'static str,
}

impl From<&Membership> for MembershipDto {
    fn from(membership: &Membership) -> Self {
        Self {
            membership_id: membership.id.as_uuid(),
            tenant_id: membership.tenant_id.as_uuid(),
            tenant_name: membership.tenant_name.clone(),
            role: membership.role.as_str(),
        }
    }
}

/// The tenant a session is scoped to.
#[derive(Debug, Serialize)]
pub struct CurrentTenantDto {
    /// The tenant's id.
    pub tenant_id: Uuid,
    /// The tenant's display name.
    pub tenant_name: String,
    /// The caller's role in it.
    pub role: &'static str,
}

impl CurrentTenantDto {
    /// The current tenant, named from `memberships`. `None` if the session is
    /// not scoped, or -- a race with a suspension between two queries -- if
    /// the membership is no longer listed.
    pub fn find(tenant: Option<SessionTenant>, memberships: &[Membership]) -> Option<Self> {
        let tenant = tenant?;
        memberships
            .iter()
            .find(|membership| membership.tenant_id == tenant.tenant_id)
            .map(|membership| Self {
                tenant_id: tenant.tenant_id.as_uuid(),
                tenant_name: membership.tenant_name.clone(),
                role: tenant.role.as_str(),
            })
    }
}

/// `POST /auth/login` response. The token itself is only in `Set-Cookie`.
#[derive(Debug, Serialize)]
pub struct SessionDto {
    /// Who logged in.
    pub user_id: Uuid,
    /// The tenant the new session is scoped to, or `null` when a tenant must
    /// be picked.
    pub tenant: Option<CurrentTenantDto>,
    /// Every active membership.
    pub memberships: Vec<MembershipDto>,
    /// When the session stops being accepted, RFC 3339 UTC.
    pub expires_at: String,
}

/// `GET /auth/me` response.
#[derive(Debug, Serialize)]
pub struct MeDto {
    /// Who the caller is.
    pub user_id: Uuid,
    /// The caller's address.
    pub email: String,
    /// The caller's display name.
    pub display_name: String,
    /// The tenant the session is scoped to, or `null`.
    pub tenant: Option<CurrentTenantDto>,
    /// Every active membership.
    pub memberships: Vec<MembershipDto>,
    /// When the session stops being accepted, RFC 3339 UTC.
    pub expires_at: String,
}

/// RFC 3339 UTC with second precision, built from the datetime's fields --
/// the same approach as the billing `api` crate, rather than enabling
/// `time`'s `formatting` feature for one call site.
pub fn rfc3339_utc(datetime: OffsetDateTime) -> String {
    let datetime = datetime.to_offset(UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        datetime.year(),
        u8::from(datetime.month()),
        datetime.day(),
        datetime.hour(),
        datetime.minute(),
        datetime.second(),
    )
}
