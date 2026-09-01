use axum::Json;
use axum::extract::State;
use domain::TenantId;

use crate::dto::SubscriptionDto;
use crate::{ApiError, AppState};

/// `GET /subscription`. The calling tenant's current subscription, or `null`.
///
/// A tenant that has never subscribed gets **200** with a `null` body, not a
/// 404. The resource addressed is "this tenant's current subscription" and it
/// exists -- its value is "none". A 404 would say the *route* was wrong and
/// force every client into error handling for the most ordinary state a fresh
/// tenant is in. Contrast `GET /invoices/{id}`, where the path names a
/// specific entity and a wrong id genuinely is not found.
pub(crate) async fn get_subscription<T>(
    tenant: T,
    State(state): State<AppState>,
) -> Result<Json<Option<SubscriptionDto>>, ApiError>
where
    T: Into<TenantId>,
{
    let subscription = state.reads.get_current_subscription(tenant.into()).await?;
    Ok(Json(subscription.map(SubscriptionDto::from)))
}
