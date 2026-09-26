/// What happened -- a closed enum that grows one variant at a time, the
/// same discipline `service`'s `Reads` and `Writes` façades already follow
/// in the billing module: grow one method (there, variant, here) at a
/// time, never arrive complete.
///
/// There is deliberately no free-form variant such as `Action::Other(&str)`.
/// That would let a caller name any action it likes, including one whose
/// name happens to embed the very secret this model exists to keep out.
/// The vocabulary is fixed by this crate, not by its callers -- which also
/// means a new auditable action is a change to this crate, not a
/// parameter a caller supplies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// A payment method was detached from a customer -- whether an
    /// explicit removal (`DELETE /payment-methods/{id}`) or a webhook
    /// confirming one Stripe already knows about. [`crate::Actor`]
    /// distinguishes who asked; this variant only says what happened.
    PaymentMethodDetached,
    /// A payment method was made a customer's default
    /// (`POST /payment-methods/{id}/default`). No webhook ever writes this
    /// column (see the billing module's own `set_default` port method), so
    /// unlike `PaymentMethodDetached` this action has exactly one caller.
    PaymentMethodSetDefault,
    /// A subscription was moved to a different local plan
    /// (`POST /subscriptions/{id}/change-plan`). Exactly one caller, the
    /// same reasoning as `PaymentMethodSetDefault`: no webhook ever writes
    /// `plan_id`.
    SubscriptionPlanChanged,
    /// A subscription was canceled (`POST /subscriptions/{id}/cancel`), at
    /// the period boundary or immediately -- whether by an explicit
    /// caller request or a webhook confirming one Stripe already knows
    /// about, the same `PaymentMethodDetached`/`Actor` split.
    SubscriptionCanceled,
    /// A Stripe SetupIntent was created (`POST /payment-methods/setup-intent`)
    /// to collect a payment method. No local row changes -- the mirror is
    /// created later, when Stripe confirms attachment via webhook -- so
    /// this entry is written standalone, not inside a business-write
    /// transaction (see `AuditSink`'s own docs on why).
    SetupIntentCreated,
    /// A Stripe Checkout Session was started
    /// (`POST /subscriptions/checkout-session`). Same standalone treatment
    /// as `SetupIntentCreated`: no local row changes until the customer
    /// completes checkout and a webhook creates the subscription.
    CheckoutSessionStarted,
    /// A subscription became (or remained) active, mirroring a
    /// `customer.subscription.*` webhook -- always `Actor::System`, since
    /// nothing here has an authenticated caller behind it.
    SubscriptionActivated,
    /// A subscription changed in some way not covered by
    /// `SubscriptionActivated` or `SubscriptionCanceled` (a period
    /// rollover, a `cancel_at_period_end` flip with no status change,
    /// `past_due`, `incomplete`).
    SubscriptionUpdated,
    /// A payment method was attached to a customer
    /// (`payment_method.attached`).
    PaymentMethodAttached,
    /// An invoice was paid in full (`invoice.paid`).
    PaymentSucceeded,
    /// An invoice payment attempt failed (`invoice.payment_failed`).
    PaymentFailed,
    /// A tenant-scoped session was issued to a user -- at login with a
    /// single membership, or when a user with several picks one. The
    /// identity module's first action. A tenant-less session (the one that
    /// exists only to pick a tenant) is not audited: it has no tenant to
    /// record it under, and the selection that follows is the auditable
    /// fact.
    SessionStarted,
    /// A user's password was changed by the user, after proving the current
    /// one. Account-level: recorded once per active membership.
    PasswordChanged,
    /// Sessions of a user were ended by something other than logout -- a
    /// password change (every session but the caller's) or a password reset
    /// (every session). Account-level. Written beside the action that caused
    /// it, with the same correlation id, so an audit reader sees that the
    /// revocation happened rather than having to infer it.
    SessionsRevoked,
    /// A password reset link was issued for an account. Recorded only when
    /// the address belongs to an active account -- there is nothing to
    /// record against otherwise -- and never visible to the requester, whose
    /// response is identical either way. Account-level, `Actor::Anonymous`.
    PasswordResetRequested,
    /// A password was set through a reset link. Account-level,
    /// `Actor::Anonymous` -- holding a link is not authenticating as the
    /// user. Always written beside a `SessionsRevoked` with the same
    /// correlation id: completing a reset ends every session.
    PasswordResetCompleted,
    /// A user changed their own profile (today: the display name). An
    /// account-level action, recorded once per tenant the user is an active
    /// member of.
    UserUpdated,
    /// An account was created by adding its address to a tenant. Recorded in
    /// that tenant only, `Actor::User` (the Owner or Admin who added it),
    /// always beside the `MembershipGranted` it came with.
    UserCreated,
    /// A user was given a role in a tenant by one of its Owners or Admins.
    /// Recorded in that tenant; the target is the membership.
    MembershipGranted,
    /// A first password was set through an invitation link. Account-level,
    /// `Actor::Anonymous`, for the reason `PasswordResetCompleted` is.
    InvitationAccepted,
    /// A membership was suspended by one of the tenant's Owners or Admins.
    /// Recorded in that tenant; the target is the membership. The member's
    /// sessions in that tenant end in the same transaction.
    MembershipSuspended,
    /// An account was deactivated -- by its owner, or by an Owner or Admin
    /// of the one tenant it belongs to. Account-level: recorded once per
    /// active membership, which for an admin-initiated deactivation is
    /// exactly one tenant, since that is the only case where an admin may
    /// do it. Always written beside a `SessionsRevoked` with the same
    /// correlation id: deactivating ends every session of the account.
    ///
    /// Distinct from `MembershipSuspended`, which removes a person from one
    /// tenant and leaves the account usable in the others. This one ends the
    /// account everywhere.
    UserDeactivated,
    /// A deactivated account was restored by an Owner or Admin of the one
    /// tenant it belongs to. Account-level. There is no self-service
    /// counterpart: a deactivated user cannot log in, so there is no session
    /// from which to ask.
    UserReactivated,
}
