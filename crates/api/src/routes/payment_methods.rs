use axum::Json;
use axum::extract::State;
use domain::TenantId;

use crate::dto::PaymentMethodDto;
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
