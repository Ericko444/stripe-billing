use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use domain::{DomainError, PaymentMethodId, TenantId};
use uuid::Uuid;

use crate::dto::{PaymentMethodDto, SetupIntentDto};
use crate::{ApiError, AppState};

/// Parses a path segment as a [`PaymentMethodId`]. A segment that is not a
/// uuid names no payment method, so it takes the same 404 path as an unknown
/// (or another tenant's) one -- never Axum's plain-text `Path` rejection.
fn parse_payment_method_id(raw: &str) -> Result<PaymentMethodId, ApiError> {
    Uuid::parse_str(raw)
        .map(PaymentMethodId::new)
        .map_err(|_| ApiError::from(DomainError::NotFound))
}

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

/// `POST /payment-methods/{id}/default`. Makes the card the calling tenant's
/// default and returns it.
///
/// Another tenant's id -- or an unknown one -- is a 404 identical to any
/// other, and `Writes::set_default_payment_method` never reaches Stripe for
/// it (§7.4: Stripe is only called after ownership is proven, and only then
/// is the mirror reconciled).
pub(crate) async fn set_default_payment_method<T>(
    tenant: T,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PaymentMethodDto>, ApiError>
where
    T: Into<TenantId>,
{
    let id = parse_payment_method_id(&id)?;
    let payment_method = state
        .writes
        .set_default_payment_method(tenant.into(), id)
        .await?;
    Ok(Json(payment_method.into()))
}

/// `DELETE /payment-methods/{id}`. Detaches the card at Stripe, then
/// soft-deletes the mirror row (§7.4, Stripe first).
///
/// **204 No Content** (Plan 4c, Open Question 2): the resource is gone and
/// the caller already holds the id it deleted, so there is nothing useful to
/// return. A second delete of the same id is a 404 -- the row is already
/// gone. Another tenant's id is a 404 with no outbound call.
pub(crate) async fn remove_payment_method<T>(
    tenant: T,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError>
where
    T: Into<TenantId>,
{
    let id = parse_payment_method_id(&id)?;
    state
        .writes
        .remove_payment_method(tenant.into(), id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
