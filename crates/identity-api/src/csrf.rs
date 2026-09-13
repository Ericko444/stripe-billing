use std::sync::Arc;

use audit::CorrelationId;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::cookie::session_token;
use crate::error::{ErrorKind, IdentityError};

/// The origins a cookie-carrying, state-changing request may come from --
/// exact `scheme://host[:port]` strings, as browsers send them in `Origin`.
#[derive(Debug, Clone)]
pub struct AllowedOrigins(Arc<Vec<HeaderValue>>);

impl AllowedOrigins {
    /// Origins from configuration. Entries that are not valid header values
    /// are dropped, so a typo narrows what is allowed rather than widening
    /// it.
    pub fn new<I, S>(origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self(Arc::new(
            origins
                .into_iter()
                .filter_map(|origin| HeaderValue::from_str(origin.as_ref().trim()).ok())
                .collect(),
        ))
    }

    fn allows(&self, origin: &HeaderValue) -> bool {
        self.0.iter().any(|allowed| allowed == origin)
    }
}

/// Middleware: refuses a state-changing request that carries the session
/// cookie unless its `Origin` is one of [`AllowedOrigins`].
///
/// The second CSRF layer; `SameSite=Strict` on the cookie is the first. It
/// keys on the **credential**, not on the path: once the session cookie
/// authenticates billing's writes too, every route that accepts it needs
/// the check, and routes that never see it -- the Stripe webhook, which
/// carries no cookie and no `Origin` -- pass untouched with no carve-out to
/// maintain. A host mounts it once, over everything it merges:
///
/// ```
/// use axum::{Router, middleware};
/// use identity_api::{AllowedOrigins, origin_check};
///
/// let allowed = AllowedOrigins::new(["http://localhost:5173"]);
/// let router: Router = Router::new()
///     // ... every router the host merges ...
///     .layer(middleware::from_fn_with_state(allowed, origin_check));
/// # let _ = router;
/// ```
///
/// A missing `Origin` on such a request is refused too. Browsers send
/// `Origin` on every cross-origin request and on same-origin `POST`,
/// `PATCH`, `PUT` and `DELETE`; a cookie-carrying write without one is not
/// coming from a page this application served.
pub async fn origin_check(
    State(allowed): State<AllowedOrigins>,
    request: Request,
    next: Next,
) -> Response {
    let safe = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    );
    if safe || session_token(request.headers()).is_none() {
        return next.run(request).await;
    }

    let origin_allowed = request
        .headers()
        .get(header::ORIGIN)
        .is_some_and(|origin| allowed.allows(origin));
    if origin_allowed {
        return next.run(request).await;
    }

    let correlation_id = request
        .extensions()
        .get::<CorrelationId>()
        .copied()
        .unwrap_or_else(|| CorrelationId::new(Uuid::new_v4()));
    IdentityError::new(ErrorKind::Forbidden, correlation_id).into_response()
}
