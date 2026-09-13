use audit::CorrelationId;
use identity_domain::{
    Clock, Email, GrantOutcome, MailPurpose, Mailer, MemberGrant, MemberRepository,
    MemberSuspension, MembershipId, OutgoingMail, PendingInvitation, Role, SessionTenant,
    TenantMember, UserId,
};
use secrecy::SecretString;
use thiserror::Error;

use crate::token::{generate_token, password_link};
use crate::{ActiveSession, INVITATION_TOKEN_LIFETIME};

/// A tenant's Owners and Admins managing its members: the minimal slice --
/// list, add, suspend.
///
/// Every operation acts on **the session's tenant** and no other: there is
/// no tenant id in any argument, so a request cannot name a tenant it is not
/// scoped to. The caller's role is the one the session lookup read from the
/// membership on this request.
pub struct MembersService<R, M, C> {
    members: R,
    mailer: M,
    clock: C,
    link_base: String,
}

/// Why a members operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MembersError {
    /// The session has no tenant, or the caller may not do this there -- a
    /// Member managing anyone, an Admin granting `owner` or suspending an
    /// Owner, anyone suspending themselves.
    #[error("not allowed")]
    Forbidden,
    /// No such membership **in the session's tenant** -- unknown, or
    /// another tenant's. One variant for both.
    #[error("no such member")]
    NotFound,
    /// The address already has a membership in this tenant.
    #[error("already a member")]
    AlreadyMember,
    /// Something this module depends on failed. The message is for logs.
    #[error("members unavailable: {0}")]
    Unavailable(String),
}

/// Whether `granter` may give someone `granted`. Exhaustive on purpose: a
/// new role has to be decided here before anything compiles.
fn may_grant(granter: Role, granted: Role) -> bool {
    match (granter, granted) {
        (Role::Owner, _) => true,
        (Role::Admin, Role::Admin | Role::Member) => true,
        (Role::Admin, Role::Owner) => false,
        (Role::Member, _) => false,
    }
}

/// Whether `caller`, with `caller_role`, may suspend `target`. Nobody
/// suspends themselves -- an Owner locking themselves out is never what was
/// meant -- and an Admin cannot suspend an Owner, for the reason an Admin
/// cannot grant `owner`. Two Owners suspending each other at once is the
/// deferred last-Owner guard's problem, named rather than handled.
fn may_suspend(caller: UserId, caller_role: Role, target: &TenantMember) -> bool {
    if target.user_id == caller {
        return false;
    }
    match (caller_role, target.role) {
        (Role::Owner, _) => true,
        (Role::Admin, Role::Admin | Role::Member) => true,
        (Role::Admin, Role::Owner) => false,
        (Role::Member, _) => false,
    }
}

/// The session's tenant, if the caller manages members there.
fn managed_tenant(session: &ActiveSession) -> Result<SessionTenant, MembersError> {
    match session.tenant {
        Some(tenant) if matches!(tenant.role, Role::Owner | Role::Admin) => Ok(tenant),
        _ => Err(MembersError::Forbidden),
    }
}

impl<R, M, C> MembersService<R, M, C>
where
    R: MemberRepository,
    M: Mailer,
    C: Clock,
{
    /// A service mailing links under `link_base` -- the frontend's origin.
    pub fn new(members: R, mailer: M, clock: C, link_base: impl Into<String>) -> Self {
        Self {
            members,
            mailer,
            clock,
            link_base: link_base.into(),
        }
    }

    /// Every membership of the session's tenant, active and suspended.
    pub async fn list(&self, session: &ActiveSession) -> Result<Vec<TenantMember>, MembersError> {
        let tenant = managed_tenant(session)?;
        self.members
            .list(tenant.tenant_id)
            .await
            .map_err(|err| MembersError::Unavailable(err.to_string()))
    }

    /// Adds `email` to the session's tenant with `role`.
    ///
    /// **The caller learns nothing about whether the address already had an
    /// account.** Both outcomes return the same shape -- the new membership
    /// -- and cost the same work: an invitation token is minted up front, one
    /// transaction runs, one mail is sent. What differs is the
    /// mail, which only the address's owner reads: a link to set a first
    /// password for an account that has none, or a notice for one that has.
    ///
    /// The membership is active at once. Adding an existing account without
    /// its consent is a named limit of this slice.
    ///
    /// A mail that cannot be sent is logged, not returned: the membership is
    /// committed, and a retry would only be refused as `AlreadyMember`. An
    /// invitee whose mail was lost recovers through "forgot password", which
    /// sets a first password as well as it replaces one.
    pub async fn add(
        &self,
        session: &ActiveSession,
        email: &Email,
        role: Role,
        correlation_id: CorrelationId,
    ) -> Result<TenantMember, MembersError> {
        let tenant = managed_tenant(session)?;
        if !may_grant(tenant.role, role) {
            return Err(MembersError::Forbidden);
        }

        let token = generate_token().map_err(|err| MembersError::Unavailable(err.to_string()))?;
        let now = self.clock.now();
        let grant = MemberGrant {
            tenant_id: tenant.tenant_id,
            email: email.clone(),
            role,
            granted_by: session.user_id,
            invitation: PendingInvitation {
                selector: token.selector(),
                verifier_hash: token.verifier().hash(),
                expires_at: now + INVITATION_TOKEN_LIFETIME,
            },
            occurred_at: now,
            correlation_id,
        };
        let (member, invitation_issued) = match self
            .members
            .grant(&grant)
            .await
            .map_err(|err| MembersError::Unavailable(err.to_string()))?
        {
            GrantOutcome::Granted {
                member,
                invitation_issued,
            } => (member, invitation_issued),
            GrantOutcome::AlreadyMember => return Err(MembersError::AlreadyMember),
        };

        let (purpose, link) = if invitation_issued {
            (
                MailPurpose::Invitation,
                password_link(&self.link_base, &token),
            )
        } else {
            (
                MailPurpose::AddedToTenant,
                SecretString::from(format!("{}/", self.link_base.trim_end_matches('/'))),
            )
        };
        let mail = OutgoingMail {
            to: member.email.clone(),
            purpose,
            link,
            correlation_id,
        };
        if let Err(err) = self.mailer.send(&mail).await {
            tracing::error!(
                correlation_id = %correlation_id.as_uuid(),
                error = %err,
                "member added, but its mail was not sent"
            );
        }
        Ok(member)
    }

    /// Suspends membership `membership_id` of the session's tenant.
    ///
    /// The membership is looked up **within the session's tenant**, so
    /// another tenant's id is `NotFound` exactly like an unknown one. The
    /// suspension, the deletion of the member's sessions in this tenant and
    /// the `MembershipSuspended` audit row are one transaction. The member's
    /// next request in this tenant is refused; their sessions in other
    /// tenants go on working.
    ///
    /// Suspending a membership that is already suspended succeeds and
    /// records nothing: the state asked for is the state it is in.
    pub async fn suspend(
        &self,
        session: &ActiveSession,
        membership_id: MembershipId,
        correlation_id: CorrelationId,
    ) -> Result<(), MembersError> {
        let tenant = managed_tenant(session)?;
        let unavailable =
            |err: identity_domain::RepositoryError| MembersError::Unavailable(err.to_string());
        let target = self
            .members
            .find(tenant.tenant_id, membership_id)
            .await
            .map_err(unavailable)?
            .ok_or(MembersError::NotFound)?;
        if !may_suspend(session.user_id, tenant.role, &target) {
            return Err(MembersError::Forbidden);
        }

        self.members
            .suspend(&MemberSuspension {
                tenant_id: tenant.tenant_id,
                membership_id,
                suspended_by: session.user_id,
                occurred_at: self.clock.now(),
                correlation_id,
            })
            .await
            .map_err(unavailable)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use identity_domain::{MembershipStatus, SessionId, SplitToken, TenantId};
    use time::Duration;
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{FakeMailer, FakeMembers, FixedClock};

    type Service = MembersService<FakeMembers, FakeMailer, FixedClock>;

    const BASE: &str = "http://localhost:5173";

    fn service(members: &FakeMembers, mailer: &FakeMailer) -> Service {
        MembersService::new(
            members.clone(),
            mailer.clone(),
            FixedClock::at_epoch_plus_days(20_000),
            BASE,
        )
    }

    fn session(tenant: Option<(TenantId, Role)>) -> ActiveSession {
        let now = FixedClock::at_epoch_plus_days(20_000).now();
        ActiveSession {
            id: SessionId::new(Uuid::new_v4()),
            user_id: UserId::new(Uuid::new_v4()),
            tenant: tenant.map(|(tenant_id, role)| SessionTenant { tenant_id, role }),
            authenticated_at: now,
            expires_at: now + Duration::hours(8),
        }
    }

    fn tenant_a() -> TenantId {
        TenantId::new(Uuid::from_u128(0xa))
    }

    fn carol() -> Result<Email, Box<dyn Error>> {
        Ok(Email::parse("carol@example.test")?)
    }

    #[tokio::test]
    async fn only_owners_and_admins_manage_members_and_admins_cannot_grant_owner()
    -> Result<(), Box<dyn Error>> {
        let members = FakeMembers::default();
        let mailer = FakeMailer::default();
        let service = service(&members, &mailer);

        let refused = [
            (None, Role::Member),
            (Some((tenant_a(), Role::Member)), Role::Member),
            (Some((tenant_a(), Role::Admin)), Role::Owner),
        ];
        for (tenant, granted) in refused {
            let caller = session(tenant);
            assert_eq!(
                service
                    .add(
                        &caller,
                        &carol()?,
                        granted,
                        CorrelationId::new(Uuid::new_v4())
                    )
                    .await
                    .err(),
                Some(MembersError::Forbidden),
                "{tenant:?} granting {granted:?}"
            );
        }
        for tenant in [None, Some((tenant_a(), Role::Member))] {
            assert_eq!(
                service.list(&session(tenant)).await.err(),
                Some(MembersError::Forbidden)
            );
        }
        assert!(members.grants().is_empty());
        assert!(mailer.sent().is_empty());

        let allowed = [
            (Role::Owner, Role::Owner),
            (Role::Owner, Role::Member),
            (Role::Admin, Role::Admin),
            (Role::Admin, Role::Member),
        ];
        for (caller_role, granted) in allowed {
            let fresh = FakeMembers::default();
            service_for(&fresh)
                .add(
                    &session(Some((tenant_a(), caller_role))),
                    &carol()?,
                    granted,
                    CorrelationId::new(Uuid::new_v4()),
                )
                .await?;
            assert_eq!(
                fresh.grants().len(),
                1,
                "{caller_role:?} granting {granted:?}"
            );
        }
        Ok(())
    }

    fn service_for(members: &FakeMembers) -> Service {
        service(members, &FakeMailer::default())
    }

    #[tokio::test]
    async fn the_grant_is_scoped_to_the_sessions_tenant_and_names_the_caller()
    -> Result<(), Box<dyn Error>> {
        let members = FakeMembers::default();
        let caller = session(Some((tenant_a(), Role::Owner)));
        let correlation_id = CorrelationId::new(Uuid::new_v4());

        service_for(&members)
            .add(&caller, &carol()?, Role::Admin, correlation_id)
            .await?;

        let grants = members.grants();
        let [grant] = grants.as_slice() else {
            return Err("expected one grant".into());
        };
        assert_eq!(grant.tenant_id, tenant_a());
        assert_eq!(grant.granted_by, caller.user_id);
        assert_eq!(grant.role, Role::Admin);
        assert_eq!(grant.correlation_id, correlation_id);
        assert_eq!(
            grant.invitation.expires_at,
            grant.occurred_at + Duration::hours(72)
        );
        Ok(())
    }

    #[tokio::test]
    async fn new_and_existing_addresses_return_the_same_shape_and_differ_only_in_the_mail()
    -> Result<(), Box<dyn Error>> {
        let caller = session(Some((tenant_a(), Role::Owner)));

        let mut results = Vec::new();
        for has_password in [false, true] {
            let members = FakeMembers::existing_account_has_password(has_password);
            let mailer = FakeMailer::default();
            let member = service(&members, &mailer)
                .add(
                    &caller,
                    &carol()?,
                    Role::Member,
                    CorrelationId::new(Uuid::new_v4()),
                )
                .await?;
            results.push((members, mailer, member));
        }
        let [(invited, invite_mail, new), (_, notice_mail, existing)] = results.as_slice() else {
            return Err("expected two results".into());
        };

        // Identical apart from ids.
        assert_eq!(
            (&new.email, new.role, new.status),
            (&existing.email, existing.role, existing.status)
        );
        assert_eq!(new.status, MembershipStatus::Active);

        let grants = invited.grants();
        let [MemberGrant { invitation, .. }] = grants.as_slice() else {
            return Err("expected one grant".into());
        };
        let invitations_sent = invite_mail.sent();
        let [(to, MailPurpose::Invitation, link, _)] = invitations_sent.as_slice() else {
            return Err("expected one invitation mail".into());
        };
        assert_eq!(to, &new.email);
        let wire = link
            .strip_prefix("http://localhost:5173/reset-password#token=")
            .ok_or("invitation link has the wrong shape")?;
        let mailed = SplitToken::parse(wire)?;
        assert_eq!(mailed.selector(), invitation.selector);
        assert!(invitation.verifier_hash.verifies(mailed.verifier()));

        let notices_sent = notice_mail.sent();
        let [(_, MailPurpose::AddedToTenant, notice_link, _)] = notices_sent.as_slice() else {
            return Err("expected one notice".into());
        };
        assert_eq!(notice_link, "http://localhost:5173/");
        assert!(!notice_link.contains("token"));
        Ok(())
    }

    #[tokio::test]
    async fn an_address_already_in_the_tenant_is_refused_and_mailed_nothing()
    -> Result<(), Box<dyn Error>> {
        let members = FakeMembers::already_member();
        let mailer = FakeMailer::default();

        let result = service(&members, &mailer)
            .add(
                &session(Some((tenant_a(), Role::Owner))),
                &carol()?,
                Role::Member,
                CorrelationId::new(Uuid::new_v4()),
            )
            .await;

        assert_eq!(result.err(), Some(MembersError::AlreadyMember));
        assert!(mailer.sent().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn a_mail_failure_does_not_fail_a_committed_grant() -> Result<(), Box<dyn Error>> {
        let members = FakeMembers::default();

        let member = service(&members, &FakeMailer::failing())
            .add(
                &session(Some((tenant_a(), Role::Owner))),
                &carol()?,
                Role::Member,
                CorrelationId::new(Uuid::new_v4()),
            )
            .await?;

        assert_eq!(member.email, carol()?);
        Ok(())
    }

    fn member(user_id: UserId, role: Role) -> Result<TenantMember, Box<dyn Error>> {
        Ok(TenantMember {
            membership_id: MembershipId::new(Uuid::new_v4()),
            user_id,
            email: Email::parse("target@example.test")?,
            role,
            status: MembershipStatus::Active,
        })
    }

    #[tokio::test]
    async fn suspension_rules_nobody_suspends_themselves_and_admins_cannot_suspend_owners()
    -> Result<(), Box<dyn Error>> {
        let cases = [
            (Role::Owner, Role::Owner, true),
            (Role::Owner, Role::Admin, true),
            (Role::Owner, Role::Member, true),
            (Role::Admin, Role::Owner, false),
            (Role::Admin, Role::Admin, true),
            (Role::Admin, Role::Member, true),
            (Role::Member, Role::Member, false),
        ];
        for (caller_role, target_role, allowed) in cases {
            let caller = session(Some((tenant_a(), caller_role)));
            let target = member(UserId::new(Uuid::new_v4()), target_role)?;
            let members = FakeMembers::with_member(target.clone());

            let result = service_for(&members)
                .suspend(
                    &caller,
                    target.membership_id,
                    CorrelationId::new(Uuid::new_v4()),
                )
                .await;

            let case = format!("{caller_role:?} suspending {target_role:?}");
            if allowed {
                assert_eq!(result, Ok(()), "{case}");
                let suspensions = members.suspensions();
                let [suspension] = suspensions.as_slice() else {
                    return Err(format!("{case}: expected one suspension").into());
                };
                assert_eq!(suspension.tenant_id, tenant_a(), "{case}");
                assert_eq!(suspension.suspended_by, caller.user_id, "{case}");
            } else {
                assert_eq!(result, Err(MembersError::Forbidden), "{case}");
                assert!(members.suspensions().is_empty(), "{case}");
            }
        }

        // Not even an Owner suspends themselves.
        let owner = session(Some((tenant_a(), Role::Owner)));
        let own = member(owner.user_id, Role::Owner)?;
        let members = FakeMembers::with_member(own.clone());
        assert_eq!(
            service_for(&members)
                .suspend(
                    &owner,
                    own.membership_id,
                    CorrelationId::new(Uuid::new_v4())
                )
                .await,
            Err(MembersError::Forbidden)
        );
        assert!(members.suspensions().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn a_membership_not_found_in_the_sessions_tenant_is_not_found()
    -> Result<(), Box<dyn Error>> {
        let members = FakeMembers::default();

        let result = service_for(&members)
            .suspend(
                &session(Some((tenant_a(), Role::Owner))),
                MembershipId::new(Uuid::new_v4()),
                CorrelationId::new(Uuid::new_v4()),
            )
            .await;

        assert_eq!(result, Err(MembersError::NotFound));
        assert!(members.suspensions().is_empty());
        assert_eq!(members.looked_up_in(), vec![tenant_a()]);
        Ok(())
    }
}
