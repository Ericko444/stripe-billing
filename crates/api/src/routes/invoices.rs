use axum::Json;
use axum::extract::{Query, State};
use domain::TenantId;

use crate::dto::{InvoicePageDto, PageParams};
use crate::{ApiError, AppState};

/// `GET /invoices`. One keyset-paginated page of the calling tenant's
/// invoices, newest first.
///
/// `?limit=` is clamped to `1..=100` (default 25), never rejected. `?after=`
/// is an opaque cursor from a previous page's `next`; a malformed one is a
/// 400 `problem+json`, never a silent restart from the top.
pub(crate) async fn list_invoices<T>(
    tenant: T,
    State(state): State<AppState>,
    Query(params): Query<PageParams>,
) -> Result<Json<InvoicePageDto>, ApiError>
where
    T: Into<TenantId>,
{
    let page = state
        .reads
        .list_invoices(tenant.into(), params.after()?, params.limit())
        .await?;
    Ok(Json(page.into()))
}
