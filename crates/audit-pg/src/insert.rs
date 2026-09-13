use audit::{Action, Actor, AuditEntry, Target};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::AuditPgError;

/// Writes `entry` to `audit.audit_log` on the given connection.
///
/// Takes `&mut PgConnection` rather than a `PgPool`, deliberately: a caller
/// that already has a transaction open for its own business write passes
/// that transaction's connection here, so the row lands in the same
/// transaction rather than in one this function opens for itself. That is
/// what lets the audit entry commit atomically with the write it
/// describes -- the composition happens in the caller, not in this crate.
///
/// A caller with no existing transaction to join uses a `PgPool`'s
/// connection directly; `&mut PgConnection` accepts either.
pub async fn insert(conn: &mut PgConnection, entry: &AuditEntry) -> Result<(), AuditPgError> {
    let (actor_kind, actor_subject_id) = actor_columns(&entry.actor());
    let (target_kind, target_id) = target_columns(&entry.target());

    sqlx::query(
        "INSERT INTO audit.audit_log \
            (id, tenant_id, actor_kind, actor_subject_id, action, target_kind, target_id, \
             occurred_at, correlation_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(Uuid::new_v4())
    .bind(entry.tenant_id().as_uuid())
    .bind(actor_kind)
    .bind(actor_subject_id)
    .bind(action_kind(&entry.action()))
    .bind(target_kind)
    .bind(target_id)
    .bind(entry.occurred_at())
    .bind(entry.correlation_id().as_uuid())
    .execute(conn)
    .await?;

    Ok(())
}

fn actor_columns(actor: &Actor) -> (&'static str, Option<Uuid>) {
    match actor {
        Actor::User(subject_id) => ("user", Some(subject_id.as_uuid())),
        Actor::System => ("system", None),
    }
}

/// A `#[non_exhaustive]`-free match: the compiler requires a new arm here
/// the moment `audit::Action` gains a variant, which is exactly the point
/// where this function has to decide that variant's column value.
fn action_kind(action: &Action) -> &'static str {
    match action {
        Action::PaymentMethodDetached => "payment_method.detached",
        Action::PaymentMethodSetDefault => "payment_method.set_default",
        Action::SubscriptionPlanChanged => "subscription.plan_changed",
        Action::SubscriptionCanceled => "subscription.canceled",
        Action::SetupIntentCreated => "setup_intent.created",
        Action::CheckoutSessionStarted => "checkout_session.started",
        Action::SubscriptionActivated => "subscription.activated",
        Action::SubscriptionUpdated => "subscription.updated",
        Action::PaymentMethodAttached => "payment_method.attached",
        Action::PaymentSucceeded => "payment.succeeded",
        Action::PaymentFailed => "payment.failed",
        Action::SessionStarted => "session.started",
        Action::UserUpdated => "user.updated",
        Action::PasswordChanged => "password.changed",
        Action::SessionsRevoked => "sessions.revoked",
    }
}

/// See [`action_kind`] -- same reasoning, for `Target`.
fn target_columns(target: &Target) -> (&'static str, Option<Uuid>) {
    match target {
        Target::PaymentMethod(id) => ("payment_method", Some(id.as_uuid())),
        Target::Subscription(id) => ("subscription", Some(id.as_uuid())),
        Target::Customer(id) => ("customer", Some(id.as_uuid())),
        Target::Plan(id) => ("plan", Some(id.as_uuid())),
        Target::Invoice(id) => ("invoice", Some(id.as_uuid())),
        Target::User(id) => ("user", Some(id.as_uuid())),
    }
}
