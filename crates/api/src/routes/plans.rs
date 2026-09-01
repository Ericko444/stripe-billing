use axum::Json;
use axum::extract::State;
use domain::TenantId;

use crate::dto::PlanDto;
use crate::{ApiError, AppState};

/// `GET /plans`. Every plan for the calling tenant.
///
/// `tenant: T` is the host's extractor and it is the **first** parameter, so
/// a reader sees the tenant before anything else and a handler missing one
/// is obvious in review. The tenant is never read from a path, query, header
/// or body -- the only way a `TenantId` reaches this function is out of `T`
/// (§8.1: accepting `tenant_id` as a request parameter is textbook IDOR).
pub(crate) async fn list_plans<T>(
    tenant: T,
    State(state): State<AppState>,
) -> Result<Json<Vec<PlanDto>>, ApiError>
where
    T: Into<TenantId>,
{
    let plans = state.reads.list_plans(tenant.into()).await?;
    Ok(Json(plans.into_iter().map(PlanDto::from).collect()))
}
