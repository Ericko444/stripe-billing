use thiserror::Error;

/// Errors representing violations of domain invariants.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    /// Attempted to combine monetary amounts in two different currencies.
    #[error("currency mismatch")]
    CurrencyMismatch,

    /// A monetary arithmetic operation overflowed.
    #[error("amount overflow")]
    AmountOverflow,

    /// The requested entity does not exist.
    #[error("not found")]
    NotFound,

    /// A persistence-layer operation failed. The message is opaque by
    /// design: `domain` must not depend on `sqlx` or any adapter's error
    /// type (S1), so adapters map their errors to this variant's `String`.
    #[error("repository error: {0}")]
    Repository(String),

    /// A `BillingProvider` operation failed. The message is opaque for the
    /// same reason as `Repository`: `domain` must not depend on the Stripe
    /// client's error type (S1), so `stripe-adapter` maps its errors to this
    /// variant's `String`.
    #[error("provider error: {0}")]
    Provider(String),

    /// A write conflicted with an existing record — for example, two
    /// concurrent attempts to reserve the same idempotency ledger row.
    #[error("conflict")]
    Conflict,

    /// A webhook signature could not be verified. Carries **no** payload by
    /// design: *why* verification failed (bad signature vs. expired
    /// timestamp vs. malformed header) is a probe oracle for anyone hitting
    /// a public, unauthenticated endpoint. The rich reason stays in the
    /// adapter's `WebhookError` and is logged server-side.
    #[error("webhook verification failed")]
    WebhookVerification,

    /// A webhook event's body did not have the shape a handler expected --
    /// for example a missing field or a subscription with no items --
    /// despite having already passed signature verification.
    ///
    /// Distinct from `WebhookVerification`: the signature is not in
    /// question here, and mapping this to that variant would tell the
    /// caller "forged or stale signature, do not retry" for a failure that
    /// is really ours (a field this code expects moved, or Stripe's account
    /// API version does not match this handler's assumption). It surfaces
    /// as a 500 at the route, which is the correct signal to retry --
    /// `init-spec.md` §5.5's error-model layering, extended to the one
    /// payload-shape failure mode webhook processing introduces. Carries a
    /// message for server-side logs, same reasoning as `Repository` and
    /// `Provider`.
    #[error("malformed webhook event: {0}")]
    MalformedEvent(String),
}
