//! Every rate limit the identity routes apply, in one place.
//!
//! Starting values, not measurements (spec D8). What each one protects
//! against is on the constant; the pairing is the point -- a limit per
//! address stops a targeted attack from many IPs, a limit per IP stops one
//! source working through many addresses, and neither does the other's job.

use identity_domain::{ClientIp, Email, RateDecision, RateKey, RateLimit};
use time::Duration;

use crate::error::{ErrorKind, IdentityError};
use crate::state::IdentityState;

/// Login attempts for one address from one client IP: guessing one account
/// from one source. Deliberately not per address alone -- a hard per-account
/// limit would let anyone lock a victim out by failing their login.
pub const LOGIN_PER_ADDRESS_AND_IP: RateLimit = RateLimit {
    max: 10,
    window: Duration::minutes(15),
};

/// Login attempts from one client IP: one source spraying many accounts.
pub const LOGIN_PER_IP: RateLimit = RateLimit {
    max: 100,
    window: Duration::minutes(15),
};

const LOGIN: &str = "login";

/// Counts a login attempt from `ip`, before the body is even parsed.
pub fn login_by_ip(
    state: &IdentityState,
    ip: ClientIp,
    correlation_id: audit::CorrelationId,
) -> Result<(), IdentityError> {
    enforce(state, &RateKey::ip(LOGIN, ip), LOGIN_PER_IP, correlation_id)
}

/// Counts a login attempt for `email` from `ip`.
pub fn login_by_address_and_ip(
    state: &IdentityState,
    email: &Email,
    ip: ClientIp,
    correlation_id: audit::CorrelationId,
) -> Result<(), IdentityError> {
    enforce(
        state,
        &RateKey::address_and_ip(LOGIN, email, ip),
        LOGIN_PER_ADDRESS_AND_IP,
        correlation_id,
    )
}

fn enforce(
    state: &IdentityState,
    key: &RateKey,
    limit: RateLimit,
    correlation_id: audit::CorrelationId,
) -> Result<(), IdentityError> {
    match state.limiter().check(key, limit) {
        RateDecision::Allowed => Ok(()),
        RateDecision::Limited { retry_after } => Err(IdentityError::new(
            ErrorKind::RateLimited {
                retry_after_secs: u64::try_from(retry_after.whole_seconds().max(1)).unwrap_or(1),
            },
            correlation_id,
        )),
    }
}
