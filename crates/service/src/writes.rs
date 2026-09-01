//! The write use cases behind Phase 4c's six mutating routes
//! (`docs/spec/phase-4c-write-routes.md`).
//!
//! The mirror of [`reads`](crate::reads): where [`Reads`](crate::Reads) is a
//! façade over the read repositories, [`Writes`] is the façade over the one
//! [`BillingProvider`] and the four repositories a mutating call touches. It
//! arrives with **no methods** and grows one per vertical slice -- the same
//! discipline `Reads` followed across Phase 4b.
//!
//! Kept a *second* port beside `Reads` rather than more methods on it (Plan
//! 4c, P1): a host test that needs a read stub should not also have to stub
//! six write methods it never calls, and the read path stays free of the
//! provider -- a [`ReadService`](crate::ReadService) still needs no Stripe
//! client, which is what keeps 4b's read tests running in a millisecond.

use async_trait::async_trait;
use domain::{
    BillingProvider, CustomerRepository, PaymentMethodRepository, PlanRepository,
    SubscriptionRepository,
};

/// Object-safe façade over the write use cases.
///
/// `api`'s `AppState` holds this as `Arc<dyn Writes>`, next to
/// `Arc<dyn Reads>` -- so `billing_router` still carries exactly one type
/// parameter and `AppState` gains exactly one field (Plan 4c, P1).
/// `#[async_trait]` for the same reason [`Reads`](crate::Reads) uses it: the
/// trait must be dyn-compatible.
///
/// **No methods yet.** Landing the port, the `AppState` field and the `demo`
/// wiring on their own means that ripple is reviewed as a diff about
/// plumbing rather than buried inside the first feature. The first method,
/// `ensure_customer`, arrives in Task 3.
#[async_trait]
pub trait Writes: Send + Sync {}

/// Holds the [`BillingProvider`] and the four repositories a mutating call
/// touches.
///
/// Generic over each port, matching [`ReadService`](crate::ReadService) and
/// `WebhookProcessor`: `demo` monomorphises the concrete `stripe-adapter`
/// and `persistence` types, and nothing is boxed on the write path. The
/// `dyn` boundary is `Arc<dyn Writes>` on `AppState`, and nowhere else
/// (Plan 4c, P2).
///
/// All five dependencies are taken from the start, like `ReadService`'s
/// four, so `AppState` and every host's wiring stay fixed as the trait
/// fills in. They are unread until Task 3 adds the first method.
#[allow(dead_code)]
pub struct WriteService<P, C, S, M, L> {
    provider: P,
    customers: C,
    subscriptions: S,
    payment_methods: M,
    plans: L,
}

impl<P, C, S, M, L> WriteService<P, C, S, M, L> {
    /// Wraps the provider and the four write repositories.
    pub fn new(provider: P, customers: C, subscriptions: S, payment_methods: M, plans: L) -> Self {
        Self {
            provider,
            customers,
            subscriptions,
            payment_methods,
            plans,
        }
    }
}

#[async_trait]
impl<P, C, S, M, L> Writes for WriteService<P, C, S, M, L>
where
    P: BillingProvider,
    C: CustomerRepository + Send + Sync,
    S: SubscriptionRepository + Send + Sync,
    M: PaymentMethodRepository + Send + Sync,
    L: PlanRepository + Send + Sync,
{
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compiles only if `Writes` is dyn-compatible -- the property the
    /// `#[async_trait]` decision exists for.
    #[allow(dead_code)]
    fn assert_dyn_compatible(_w: &dyn Writes) {}
}
