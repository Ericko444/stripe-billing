use axum::Json;
use axum::extract::{Path, Query, State};
use domain::{DomainError, InvoiceId, TenantId};
use uuid::Uuid;

use crate::dto::{InvoiceDto, InvoicePageDto, PageParams};
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

/// `GET /invoices/{id}`. One invoice, scoped to the calling tenant.
///
/// An unknown id and another tenant's id both return an **identical** 404
/// (correlation id aside): the repository's tenant-scoped `find` returns
/// `None` for both, and the error mapping gives `DomainError::NotFound` one
/// response. Returning 403 for "exists but not yours" would make the status
/// code an id-enumeration oracle.
///
/// The `{id}` segment is read as a string and parsed here; a segment that is
/// not a uuid names no invoice, so it takes the same 404 path rather than
/// Axum's plain-text `Path` rejection.
pub(crate) async fn get_invoice<T>(
    tenant: T,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<InvoiceDto>, ApiError>
where
    T: Into<TenantId>,
{
    let id = Uuid::parse_str(&id)
        .map(InvoiceId::new)
        .map_err(|_| ApiError::from(DomainError::NotFound))?;
    let invoice = state.reads.get_invoice(tenant.into(), id).await?;
    Ok(Json(invoice.into()))
}
