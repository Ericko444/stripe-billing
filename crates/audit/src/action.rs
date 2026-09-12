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
}
