use domain::TenantId;
use uuid::Uuid;

/// Identifies the request behind a [`Writes`](crate::Writes) call, replacing
/// the bare `TenantId` every method took before.
///
/// `actor` is deliberately not a field here. This module has no user
/// model at all -- a host's tenant extractor produces only a `TenantId`,
/// never a user id (see the project's own documented boundary: no
/// authentication, no user management here) -- so an audit entry for a
/// billing write is always `audit::Actor::System`, unconditionally. A
/// per-request field would carry no information; whatever constructs the
/// `AuditEntry` sets that constant itself rather than reading it from here.
///
/// `correlation_id` is a raw `Uuid`, not `audit::CorrelationId`: this crate
/// does not depend on `audit` (nothing here constructs an `AuditEntry`
/// yet), and `Uuid` is the neutral type both `api` and `audit` already
/// wrap in their own newtypes. Each converts at its own boundary -- the
/// same pattern already used for tenant and subject ids between this
/// module and `audit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestContext {
    /// The tenant the call is scoped to.
    pub tenant_id: TenantId,
    /// This request's correlation id, minted once by `api`'s middleware.
    pub correlation_id: Uuid,
}

impl RequestContext {
    /// Builds a context from the caller's tenant id and this request's
    /// correlation id.
    pub fn new(tenant_id: TenantId, correlation_id: Uuid) -> Self {
        Self {
            tenant_id,
            correlation_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn carries_the_tenant_and_correlation_id_it_was_built_with() {
        let tenant_id = TenantId::new(Uuid::new_v4());
        let correlation_id = Uuid::new_v4();

        let ctx = RequestContext::new(tenant_id, correlation_id);

        assert_eq!(ctx.tenant_id, tenant_id);
        assert_eq!(ctx.correlation_id, correlation_id);
    }
}
