use domain::{
    DomainError, OutboundRequest, OutboundRequestId, OutboundRequestRepository, TenantId,
};
use stripe::IdempotencyKey;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::StripeError;

/// How long a persisted idempotency key is trusted to still be live at
/// Stripe. Stripe documents roughly 24 hours; this stays a little under
/// that so the ledger never believes a key is still valid after Stripe has
/// already dropped it -- the unsafe direction here is being wrong
/// optimistically, so the margin errs toward treating a key as expired
/// slightly early rather than slightly late.
const KEY_WINDOW: Duration = Duration::hours(23);

/// A reserved idempotency key, ready to send to Stripe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    /// The ledger row's id, needed to mark it complete afterward.
    pub id: OutboundRequestId,
    /// The key to send to Stripe on this attempt.
    pub idempotency_key: String,
}

/// Turns a reservation's key string into the SDK's [`IdempotencyKey`].
///
/// Fallible on purpose: the key came from the ledger, which mints a v4 uuid
/// and so is always valid in practice -- but the ledger's key column is
/// plain `TEXT`, so a malformed value stays a typed error rather than an
/// `unwrap` (which the workspace lints deny anyway).
///
/// Lives here, next to [`Reservation`], rather than in one adapter module:
/// every module that sends a mutating call needs it (Phase 4c Task 4 moved
/// it out of `subscriptions.rs`).
pub(crate) fn idempotency_key(reservation: &Reservation) -> Result<IdempotencyKey, DomainError> {
    IdempotencyKey::new(&reservation.idempotency_key)
        .map_err(|err| StripeError::Config(err.to_string()))
        .map_err(DomainError::from)
}

/// The idempotency ledger's reserve/complete state machine. Knows nothing
/// about Stripe -- it hands back a key to send and takes back the object id
/// a successful call produced. `StripeBillingProvider` is the only caller
/// that talks to Stripe; this type only talks to the ledger.
///
/// Generic over `R` rather than holding `Arc<dyn OutboundRequestRepository>`:
/// every repository port in this workspace uses native `async fn`, which is
/// not dyn-compatible. This is exactly the correction recorded at the end of
/// `docs/intent/phase-2.md` -- the Phase 1 plan filed it as a Phase 4 risk,
/// and it actually lands here.
pub struct Ledger<R: OutboundRequestRepository> {
    repo: R,
}

impl<R: OutboundRequestRepository> Ledger<R> {
    /// Wraps a repository as a `Ledger`.
    pub fn new(repo: R) -> Self {
        Self { repo }
    }

    /// Reserves an idempotency key for `(tenant_id, operation, fingerprint)`,
    /// implementing the six-case reserve state machine
    /// (`docs/spec/phase-2-stripe-adapter.md`, "Resolved decisions §2"):
    ///
    /// - **No row, or a completed row past the key window** (cases A, D):
    ///   insert a fresh key. A fingerprint recurring after its earlier
    ///   attempt's key has aged out is a genuinely new operation, not a
    ///   retry.
    /// - **A row still within the key window, complete or not** (cases B,
    ///   C): reuse its key. Calling Stripe again with a still-live key is
    ///   safe either way -- Stripe's own dedup returns the original result
    ///   for a completed attempt, and an in-flight attempt's retry is
    ///   exactly what the key exists to make safe.
    /// - **An incomplete row past the key window** (case E): no safe local
    ///   resolution. Reusing the key no longer dedups at Stripe; minting a
    ///   fresh one risks a duplicate if the original attempt actually
    ///   landed. Returns a typed error naming reconciliation, and performs
    ///   no further work -- deliberately not a guess.
    /// - **Losing the race to insert** (case F): another reserve for the
    ///   same fingerprint won first (Postgres's unique violation surfaces
    ///   here as `DomainError::Conflict`, per the `0009` migration's partial
    ///   index). Re-reads and reuses the winner's key -- continues as case B.
    pub async fn reserve(
        &self,
        tenant_id: TenantId,
        operation: &str,
        fingerprint: &str,
    ) -> Result<Reservation, DomainError> {
        match self
            .repo
            .find_by_fingerprint(tenant_id, operation, fingerprint)
            .await?
        {
            // Cases B and C: a live row exists, complete or not -- reuse it.
            Some(row) if !Self::expired(&row) => Ok(Reservation {
                id: row.id,
                idempotency_key: row.idempotency_key,
            }),
            // Case E: incomplete, and its key has already expired at
            // Stripe. No safe local action.
            Some(row) if row.completed_at.is_none() => Err(DomainError::Provider(format!(
                "outbound request {} is incomplete and its idempotency key has \
                 expired; reconcile against Stripe before retrying",
                row.id.as_uuid()
            ))),
            // Cases A and D: no row, or a completed row whose key has aged
            // out -- either way, this is (or has become) a fresh attempt.
            _ => {
                self.insert_reservation(tenant_id, operation, fingerprint)
                    .await
            }
        }
    }

    /// Marks a reservation complete, recording the Stripe object id the
    /// call produced. Safe to call again on an already-complete reservation
    /// (case C's re-call re-records the same completion) -- the underlying
    /// `mark_complete` is unconditional on the row's current state.
    pub async fn complete(
        &self,
        tenant_id: TenantId,
        reservation: &Reservation,
        stripe_object_id: String,
    ) -> Result<(), DomainError> {
        self.repo
            .mark_complete(tenant_id, reservation.id, stripe_object_id)
            .await?;
        Ok(())
    }

    async fn insert_reservation(
        &self,
        tenant_id: TenantId,
        operation: &str,
        fingerprint: &str,
    ) -> Result<Reservation, DomainError> {
        let key = Uuid::new_v4().to_string();
        match self
            .repo
            .create(
                tenant_id,
                operation.to_string(),
                fingerprint.to_string(),
                key,
            )
            .await
        {
            Ok(row) => Ok(Reservation {
                id: row.id,
                idempotency_key: row.idempotency_key,
            }),
            // Case F: lost the race to another reserve for the same
            // fingerprint. Re-read and reuse whichever key won.
            Err(DomainError::Conflict) => {
                let row = self
                    .repo
                    .find_by_fingerprint(tenant_id, operation, fingerprint)
                    .await?
                    .ok_or_else(|| {
                        DomainError::Provider(
                            "lost an insert race but found no row on re-read".to_string(),
                        )
                    })?;
                Ok(Reservation {
                    id: row.id,
                    idempotency_key: row.idempotency_key,
                })
            }
            Err(other) => Err(other),
        }
    }

    fn expired(row: &OutboundRequest) -> bool {
        OffsetDateTime::now_utc() - row.created_at > KEY_WINDOW
    }
}
