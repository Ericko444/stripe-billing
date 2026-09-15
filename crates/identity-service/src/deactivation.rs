//! Closing an account, and reopening one.
//!
//! Separate from [`members`](crate::members) because it is a different kind
//! of act. Suspension removes a person from **one** tenant and leaves the
//! account working in the others; deactivation ends the account
//! **everywhere**. That difference is the whole reason this module's
//! authorization rule is not the members rule.

use audit::{Action, Actor, CorrelationId, SubjectId, Target, TargetId};
use identity_domain::{
    AccountEvent, Clock, DeactivateOutcome, MemberRepository, MembershipId, TenantId, UserId,
    UserRepository,
};

use crate::members::{managed_tenant, may_suspend};
use crate::{ActiveSession, MembersError};

/// Deactivating and reactivating accounts.
pub struct DeactivationService<R, U, C> {
    members: R,
    users: U,
    clock: C,
}

impl<R, U, C> DeactivationService<R, U, C>
where
    R: MemberRepository,
    U: UserRepository,
    C: Clock,
{
    /// A service over the members of a tenant and the accounts behind them.
    pub fn new(members: R, users: U, clock: C) -> Self {
        Self {
            members,
            users,
            clock,
        }
    }

    /// Deactivates the account behind a membership of the session's tenant.
    ///
    /// # Why a tenant may not always do this
    ///
    /// Roles are tenant-scoped; deactivation is not. An Owner of one tenant
    /// ending an account that also belongs to another would reach across a
    /// boundary this module exists to hold -- and could lock a second tenant
    /// out of its own Owner. So the repository is asked for the narrow
    /// power: deactivate **only if** this tenant is the account's one active
    /// membership. An account that lives anywhere else is refused, and the
    /// tenant-scoped tool for that case is
    /// [`suspend`](crate::MembersService::suspend), which was always the
    /// right instrument for "not in my tenant any more".
    ///
    /// # Why the refusals are indistinguishable
    ///
    /// "You may not" and "they belong to another tenant" are both
    /// [`MembersError::Forbidden`]. Separating them would tell an Admin of
    /// one tenant that the target also belongs to some other tenant -- a
    /// membership of a tenant they have no part in. The caller learns only
    /// that it did not happen.
    ///
    /// Deactivating oneself goes through
    /// [`deactivate_self`](Self::deactivate_self), not here: `may_suspend`
    /// refuses a caller targeting their own membership, so an Owner cannot
    /// close their own account by this route and call it administration.
    pub async fn deactivate_member(
        &self,
        session: &ActiveSession,
        membership_id: MembershipId,
        correlation_id: CorrelationId,
    ) -> Result<(), MembersError> {
        let tenant = managed_tenant(session)?;
        let target = self.target(tenant.tenant_id, membership_id).await?;
        if !may_suspend(session.user_id, tenant.role, &target) {
            return Err(MembersError::Forbidden);
        }

        let events = self.deactivation_events(
            target.user_id,
            Actor::User(subject(session.user_id)),
            correlation_id,
        );
        match self
            .users
            .deactivate(target.user_id, Some(tenant.tenant_id), &events)
            .await
            .map_err(unavailable)?
        {
            // Already deactivated is success: the state asked for is the
            // state it is in, the same contract as suspending twice.
            DeactivateOutcome::Deactivated | DeactivateOutcome::AlreadyDeactivated => Ok(()),
            DeactivateOutcome::BelongsToOtherTenants => Err(MembersError::Forbidden),
        }
    }

    /// Restores a deactivated account behind a membership of the session's
    /// tenant.
    ///
    /// There is no self-service counterpart, and cannot be: a deactivated
    /// user cannot log in, so there is no session from which to ask. That
    /// asymmetry is why deactivation is not a destructive act an admin can
    /// perform with no way back.
    ///
    /// No sole-tenant constraint. Reactivating grants no access the account
    /// did not already have -- its memberships are exactly as they were --
    /// so it cannot reach into another tenant the way deactivating can.
    pub async fn reactivate_member(
        &self,
        session: &ActiveSession,
        membership_id: MembershipId,
        correlation_id: CorrelationId,
    ) -> Result<(), MembersError> {
        let tenant = managed_tenant(session)?;
        let target = self.target(tenant.tenant_id, membership_id).await?;
        if !may_suspend(session.user_id, tenant.role, &target) {
            return Err(MembersError::Forbidden);
        }

        let event = AccountEvent {
            actor: Actor::User(subject(session.user_id)),
            action: Action::UserReactivated,
            target: Target::User(TargetId::new(target.user_id.as_uuid())),
            occurred_at: self.clock.now(),
            correlation_id,
        };
        self.users
            .reactivate(target.user_id, &event)
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    /// Closes the caller's own account, in every tenant.
    ///
    /// No sole-tenant constraint: there is no other tenant's interest to
    /// protect from a person choosing to leave. Every session ends, this one
    /// included, so the caller is logged out by their own request.
    ///
    /// The actor is the caller themselves, not `Anonymous`: they proved a
    /// session to get here.
    pub async fn deactivate_self(
        &self,
        session: &ActiveSession,
        correlation_id: CorrelationId,
    ) -> Result<(), MembersError> {
        let events = self.deactivation_events(
            session.user_id,
            Actor::User(subject(session.user_id)),
            correlation_id,
        );
        match self
            .users
            .deactivate(session.user_id, None, &events)
            .await
            .map_err(unavailable)?
        {
            DeactivateOutcome::Deactivated | DeactivateOutcome::AlreadyDeactivated => Ok(()),
            // Unreachable with `None`, which lifts the constraint; refusing
            // rather than asserting keeps it off a panicking path.
            DeactivateOutcome::BelongsToOtherTenants => Err(MembersError::Forbidden),
        }
    }

    /// The membership, if it is one of `tenant_id`'s.
    async fn target(
        &self,
        tenant_id: TenantId,
        membership_id: MembershipId,
    ) -> Result<identity_domain::TenantMember, MembersError> {
        self.members
            .find(tenant_id, membership_id)
            .await
            .map_err(unavailable)?
            .ok_or(MembersError::NotFound)
    }

    /// The pair of rows a deactivation writes: what happened, and that every
    /// session went with it. Written together with one correlation id, so a
    /// reader sees the revocation rather than having to infer it -- the same
    /// shape a password reset uses.
    fn deactivation_events(
        &self,
        user_id: UserId,
        actor: Actor,
        correlation_id: CorrelationId,
    ) -> Vec<AccountEvent> {
        let occurred_at = self.clock.now();
        let target = Target::User(TargetId::new(user_id.as_uuid()));
        vec![
            AccountEvent {
                actor,
                action: Action::UserDeactivated,
                target,
                occurred_at,
                correlation_id,
            },
            AccountEvent {
                actor,
                action: Action::SessionsRevoked,
                target,
                occurred_at,
                correlation_id,
            },
        ]
    }
}

fn subject(user_id: UserId) -> SubjectId {
    SubjectId::new(user_id.as_uuid())
}

fn unavailable(err: identity_domain::RepositoryError) -> MembersError {
    MembersError::Unavailable(err.to_string())
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use identity_domain::{Email, MembershipStatus, Role, SessionId, TenantMember, User};
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{FakeMembers, FakeUsers, FixedClock};
    use identity_domain::SessionTenant;

    fn tenant_a() -> TenantId {
        TenantId::new(Uuid::from_u128(0xa))
    }

    fn tenant_b() -> TenantId {
        TenantId::new(Uuid::from_u128(0xb))
    }

    fn session(user_id: UserId, tenant: Option<(TenantId, Role)>) -> ActiveSession {
        let now = FixedClock::at_epoch_plus_days(20_000).now();
        ActiveSession {
            id: SessionId::new(Uuid::new_v4()),
            user_id,
            tenant: tenant.map(|(tenant_id, role)| SessionTenant { tenant_id, role }),
            authenticated_at: now,
            expires_at: now + Duration::hours(8),
        }
    }

    fn membership_id() -> MembershipId {
        MembershipId::new(Uuid::from_u128(0x77))
    }

    fn member(user_id: UserId, role: Role) -> Result<TenantMember, Box<dyn Error>> {
        Ok(TenantMember {
            membership_id: membership_id(),
            user_id,
            email: Email::parse("carol@example.test")?,
            role,
            status: MembershipStatus::Active,
        })
    }

    fn account(user_id: UserId) -> Result<User, Box<dyn Error>> {
        Ok(User {
            id: user_id,
            email: Email::parse("carol@example.test")?,
            display_name: String::new(),
            password_hash: None,
            deactivated_at: None,
        })
    }

    type Service = DeactivationService<FakeMembers, FakeUsers, FixedClock>;

    fn service(members: FakeMembers, users: FakeUsers) -> Service {
        DeactivationService::new(members, users, FixedClock::at_epoch_plus_days(20_000))
    }

    /// An Owner or Admin may close an account that lives entirely inside
    /// their tenant, audited as `UserDeactivated` beside a `SessionsRevoked`
    /// sharing its correlation id.
    #[tokio::test]
    async fn an_admin_may_deactivate_an_account_that_is_only_in_their_tenant()
    -> Result<(), Box<dyn Error>> {
        let carol = UserId::new(Uuid::new_v4());
        let users = FakeUsers::with(vec![account(carol)?]).in_tenants(carol, &[tenant_a()]);
        let service = service(
            FakeMembers::with_member(member(carol, Role::Member)?),
            users.clone(),
        );
        let caller = session(UserId::new(Uuid::new_v4()), Some((tenant_a(), Role::Admin)));
        let correlation_id = CorrelationId::new(Uuid::new_v4());

        service
            .deactivate_member(&caller, membership_id(), correlation_id)
            .await?;

        let events = users.account_events();
        let actions: Vec<Action> = events.iter().map(|(_, event)| event.action).collect();
        assert_eq!(
            actions,
            vec![Action::UserDeactivated, Action::SessionsRevoked]
        );
        assert!(events.iter().all(|(user, event)| *user == carol
            && event.correlation_id == correlation_id
            && event.actor == Actor::User(subject(caller.user_id))));
        Ok(())
    }

    /// The rule the module turns on: a tenant cannot end an account that
    /// also lives somewhere else. It gets the same `Forbidden` as any other
    /// refusal -- saying *why* would disclose the other membership.
    #[tokio::test]
    async fn an_account_in_a_second_tenant_is_refused_indistinguishably()
    -> Result<(), Box<dyn Error>> {
        let carol = UserId::new(Uuid::new_v4());
        let users =
            FakeUsers::with(vec![account(carol)?]).in_tenants(carol, &[tenant_a(), tenant_b()]);
        let service = service(
            FakeMembers::with_member(member(carol, Role::Member)?),
            users.clone(),
        );
        let caller = session(UserId::new(Uuid::new_v4()), Some((tenant_a(), Role::Owner)));

        let refused = service
            .deactivate_member(&caller, membership_id(), CorrelationId::new(Uuid::new_v4()))
            .await;

        assert_eq!(refused.err(), Some(MembersError::Forbidden));
        assert!(users.account_events().is_empty());

        // The very same error a Member gets for having no business here at
        // all: the two refusals are one answer.
        let powerless = session(
            UserId::new(Uuid::new_v4()),
            Some((tenant_a(), Role::Member)),
        );
        let also_refused = service
            .deactivate_member(
                &powerless,
                membership_id(),
                CorrelationId::new(Uuid::new_v4()),
            )
            .await;
        assert_eq!(also_refused.err(), Some(MembersError::Forbidden));
        Ok(())
    }

    /// Members manage nobody, Admins cannot reach an Owner, and a session
    /// with no tenant picked cannot do this at all.
    #[tokio::test]
    async fn only_owners_and_admins_deactivate_and_admins_cannot_reach_an_owner()
    -> Result<(), Box<dyn Error>> {
        let carol = UserId::new(Uuid::new_v4());
        let users = FakeUsers::with(vec![account(carol)?]).in_tenants(carol, &[tenant_a()]);

        for (tenant, target_role) in [
            (None, Role::Member),
            (Some((tenant_a(), Role::Member)), Role::Member),
            (Some((tenant_a(), Role::Admin)), Role::Owner),
        ] {
            let service = service(
                FakeMembers::with_member(member(carol, target_role)?),
                users.clone(),
            );
            let caller = session(UserId::new(Uuid::new_v4()), tenant);
            assert_eq!(
                service
                    .deactivate_member(&caller, membership_id(), CorrelationId::new(Uuid::new_v4()))
                    .await
                    .err(),
                Some(MembersError::Forbidden),
                "{tenant:?} -> {target_role:?}"
            );
        }
        assert!(users.account_events().is_empty());
        Ok(())
    }

    /// Nobody closes their own account through the administrative route --
    /// that is what `deactivate_self` is for.
    #[tokio::test]
    async fn an_admin_cannot_deactivate_themselves_through_the_members_route()
    -> Result<(), Box<dyn Error>> {
        let alice = UserId::new(Uuid::new_v4());
        let users = FakeUsers::with(vec![account(alice)?]).in_tenants(alice, &[tenant_a()]);
        let service = service(
            FakeMembers::with_member(member(alice, Role::Owner)?),
            users.clone(),
        );
        let caller = session(alice, Some((tenant_a(), Role::Owner)));

        let refused = service
            .deactivate_member(&caller, membership_id(), CorrelationId::new(Uuid::new_v4()))
            .await;

        assert_eq!(refused.err(), Some(MembersError::Forbidden));
        assert!(users.account_events().is_empty());
        Ok(())
    }

    /// Self-service carries no sole-tenant constraint: a person may leave,
    /// however many tenants they belong to.
    #[tokio::test]
    async fn a_user_may_close_their_own_account_in_however_many_tenants()
    -> Result<(), Box<dyn Error>> {
        let alice = UserId::new(Uuid::new_v4());
        let users =
            FakeUsers::with(vec![account(alice)?]).in_tenants(alice, &[tenant_a(), tenant_b()]);
        let service = service(FakeMembers::default(), users.clone());
        let caller = session(alice, Some((tenant_a(), Role::Member)));
        let correlation_id = CorrelationId::new(Uuid::new_v4());

        service.deactivate_self(&caller, correlation_id).await?;

        let actions: Vec<Action> = users
            .account_events()
            .iter()
            .map(|(_, event)| event.action)
            .collect();
        assert_eq!(
            actions,
            vec![Action::UserDeactivated, Action::SessionsRevoked]
        );
        Ok(())
    }

    /// Reactivation is the symmetric admin act, audited as its own action.
    #[tokio::test]
    async fn an_admin_may_reactivate_and_it_is_recorded() -> Result<(), Box<dyn Error>> {
        let carol = UserId::new(Uuid::new_v4());
        let mut deactivated = account(carol)?;
        deactivated.deactivated_at = Some(OffsetDateTime::UNIX_EPOCH);
        let users = FakeUsers::with(vec![deactivated]).in_tenants(carol, &[tenant_a()]);
        let service = service(
            FakeMembers::with_member(member(carol, Role::Member)?),
            users.clone(),
        );
        let caller = session(UserId::new(Uuid::new_v4()), Some((tenant_a(), Role::Admin)));
        let correlation_id = CorrelationId::new(Uuid::new_v4());

        service
            .reactivate_member(&caller, membership_id(), correlation_id)
            .await?;

        let events = users.account_events();
        let [(user, event)] = events.as_slice() else {
            return Err("expected one event".into());
        };
        assert_eq!(*user, carol);
        assert_eq!(event.action, Action::UserReactivated);
        assert_eq!(event.correlation_id, correlation_id);
        Ok(())
    }

    /// A membership that is not this tenant's is `NotFound`, exactly like an
    /// unknown id -- the members route's existing contract.
    #[tokio::test]
    async fn another_tenants_membership_is_not_found() -> Result<(), Box<dyn Error>> {
        let users = FakeUsers::default();
        let service = service(FakeMembers::default(), users.clone());
        let caller = session(UserId::new(Uuid::new_v4()), Some((tenant_a(), Role::Owner)));

        let refused = service
            .deactivate_member(&caller, membership_id(), CorrelationId::new(Uuid::new_v4()))
            .await;

        assert_eq!(refused.err(), Some(MembersError::NotFound));
        assert!(users.account_events().is_empty());
        Ok(())
    }
}
