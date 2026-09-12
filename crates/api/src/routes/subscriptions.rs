use axum::Json;
use axum::extract::{Extension, Path, State};
use domain::{DomainError, PlanId, SubscriptionId, TenantId};
use uuid::Uuid;

use crate::dto::{
    CancelRequest, ChangePlanRequest, CheckoutSessionDto, CheckoutSessionRequest, SubscriptionDto,
};
use crate::{ApiError, AppState, CorrelationId};

/// Parses a path segment as a [`SubscriptionId`]. A segment that is not a
/// uuid names no subscription, so it takes the same 404 path as an unknown
/// (or another tenant's) one -- never Axum's plain-text `Path` rejection.
fn parse_subscription_id(raw: &str) -> Result<SubscriptionId, ApiError> {
    Uuid::parse_str(raw)
        .map(SubscriptionId::new)
        .map_err(|_| ApiError::from(DomainError::NotFound))
}

/// Parses the request body's `plan_id` as a [`PlanId`]. Same reasoning as
/// `parse_subscription_id`: a malformed uuid names no plan, so it is a 404,
/// matching what an unknown-but-valid plan id also gets.
fn parse_plan_id(raw: &str) -> Result<PlanId, ApiError> {
    Uuid::parse_str(raw)
        .map(PlanId::new)
        .map_err(|_| ApiError::from(DomainError::NotFound))
}

/// `POST /subscriptions/{id}/change-plan`. Changes the calling tenant's
/// subscription to `{plan_id}` (a **local** plan id) and returns its
/// current state.
///
/// Another tenant's subscription id -- or an unknown plan id -- is a 404
/// identical to an unknown subscription id, and the provider is never
/// called for either: `Writes::change_plan` resolves both against the
/// tenant-scoped repositories before it ever reaches Stripe.
pub(crate) async fn change_plan<T>(
    tenant: T,
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Path(id): Path<String>,
    Json(body): Json<ChangePlanRequest>,
) -> Result<Json<SubscriptionDto>, ApiError>
where
    T: Into<TenantId>,
{
    let id = parse_subscription_id(&id)?;
    let plan_id = parse_plan_id(&body.plan_id)?;
    let subscription = state
        .writes
        .change_plan(tenant.into(), id, plan_id)
        .await
        .map_err(|err| ApiError::from(err).with_correlation_id(correlation_id))?;
    Ok(Json(subscription.into()))
}

/// `POST /subscriptions/{id}/cancel`. Cancels the calling tenant's
/// subscription, at the period boundary or immediately, and returns its
/// current state.
///
/// Another tenant's subscription id is a 404 identical to an unknown one,
/// checked before any outbound call. Cancelling an already-canceled
/// subscription is a 200 with the current (unchanged) state, not an error --
/// see `Writes::cancel_subscription`'s own docs.
pub(crate) async fn cancel_subscription<T>(
    tenant: T,
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Path(id): Path<String>,
    Json(body): Json<CancelRequest>,
) -> Result<Json<SubscriptionDto>, ApiError>
where
    T: Into<TenantId>,
{
    let id = parse_subscription_id(&id)?;
    let subscription = state
        .writes
        .cancel_subscription(tenant.into(), id, body.at_period_end)
        .await
        .map_err(|err| ApiError::from(err).with_correlation_id(correlation_id))?;
    Ok(Json(subscription.into()))
}

/// `POST /subscriptions/checkout-session`. Starts a `subscription`-mode
/// Stripe Checkout Session for `{plan_id}` (a **local** id) and returns the
/// hosted-page `url` the frontend redirects to.
///
/// An unknown or another tenant's `plan_id` is a 404, checked before any
/// outbound call. The success/cancel URLs come from `AppState`'s
/// `checkout_urls` (host config), never the request. This handler logs
/// nothing -- the returned `url` travels only in the response body.
pub(crate) async fn start_checkout_session<T>(
    tenant: T,
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Json(body): Json<CheckoutSessionRequest>,
) -> Result<Json<CheckoutSessionDto>, ApiError>
where
    T: Into<TenantId>,
{
    let plan_id = parse_plan_id(&body.plan_id)?;
    let snapshot = state
        .writes
        .start_checkout_session(
            tenant.into(),
            plan_id,
            &state.checkout_urls.success,
            &state.checkout_urls.cancel,
        )
        .await
        .map_err(|err| ApiError::from(err).with_correlation_id(correlation_id))?;
    Ok(Json(snapshot.into()))
}
