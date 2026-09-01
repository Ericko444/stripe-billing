use axum::Json;
use axum::extract::State;
use domain::TenantId;

use crate::dto::{PaymentMethodDto, SetupIntentDto};
use crate::{ApiError, AppState};

/// `GET /payment-methods`. Every stored card for the calling tenant.
///
/// Soft-deleted rows are already excluded by the repository. The DTO carries
/// display metadata only (§7.4): brand, last4, default flag -- never card
/// data, which lives in Stripe.
pub(crate) async fn list_payment_methods<T>(
    tenant: T,
    State(state): State<AppState>,
) -> Result<Json<Vec<PaymentMethodDto>>, ApiError>
where
    T: Into<TenantId>,
{
    let payment_methods = state.reads.list_payment_methods(tenant.into()).await?;
    Ok(Json(
        payment_methods
            .into_iter()
            .map(PaymentMethodDto::from)
            .collect(),
    ))
}

/// `POST /payment-methods/setup-intent`. Starts payment-method collection
/// for the calling tenant.
///
/// Resolves (or creates) the tenant's Stripe customer via
/// `Writes::ensure_customer`, then asks Stripe for a SetupIntent. A tenant
/// that has never had a customer still succeeds.
///
/// The response body carries a `client_secret` the frontend hands to
/// Stripe.js. That value is bearer-ish and appears **only** in this body --
/// this handler logs nothing, and the DTO is the one place it travels (§9).
pub(crate) async fn create_setup_intent<T>(
    tenant: T,
    State(state): State<AppState>,
) -> Result<Json<SetupIntentDto>, ApiError>
where
    T: Into<TenantId>,
{
    let snapshot = state.writes.create_setup_intent(tenant.into()).await?;
    Ok(Json(SetupIntentDto::from(snapshot)))
}
