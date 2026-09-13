//! Every rate limit the identity routes apply, in one place.
//!
//! Starting values, not measurements (spec D8). What each one protects
//! against is on the constant; the pairing is the point -- a limit per
//! address stops a targeted attack from many IPs, a limit per IP stops one
//! source working through many addresses, and neither does the other's job.

use identity_domain::{ClientIp, Email, RateDecision, RateKey, RateLimit, RateLimiter};
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

/// Reset requests for one address: mail-bombing a victim, or spraying one
/// account's reset from many IPs. Over it, the request is dropped **silently**
/// -- still `202`, nothing sent -- so the limit is no signal about anything.
/// Keyed on the address whether or not an account exists; a limit keyed on
/// the account would itself say which addresses are accounts.
pub const RESET_REQUEST_PER_ADDRESS: RateLimit = RateLimit {
    max: 3,
    window: Duration::hours(1),
};

/// Reset requests from one client IP: one source working through many
/// addresses. Over it: `429`, which says nothing about any address.
pub const RESET_REQUEST_PER_IP: RateLimit = RateLimit {
    max: 20,
    window: Duration::hours(1),
};

/// Reset completions from one client IP. A verifier is 256 random bits, so
/// this is not what stops guessing -- it bounds how much Argon2 hashing a
/// single source can make the server do with well-formed links, and keeps a
/// flood of attempts visible.
pub const RESET_COMPLETE_PER_IP: RateLimit = RateLimit {
    max: 20,
    window: Duration::minutes(15),
};

const LOGIN: &str = "login";
const RESET_COMPLETE: &str = "reset-complete";

/// Counts a reset completion from `ip`; `429` when over.
pub fn reset_complete_by_ip(
    limiter: &dyn RateLimiter,
    ip: ClientIp,
    correlation_id: audit::CorrelationId,
) -> Result<(), IdentityError> {
    decide(
        limiter.check(&RateKey::ip(RESET_COMPLETE, ip), RESET_COMPLETE_PER_IP),
        correlation_id,
    )
}
const RESET_REQUEST: &str = "reset-request";

/// Counts a reset request from `ip`; `429` when over.
pub fn reset_request_by_ip(
    limiter: &dyn RateLimiter,
    ip: ClientIp,
    correlation_id: audit::CorrelationId,
) -> Result<(), IdentityError> {
    decide(
        limiter.check(&RateKey::ip(RESET_REQUEST, ip), RESET_REQUEST_PER_IP),
        correlation_id,
    )
}

/// Counts a reset request for `email`; `false` when over -- for the caller
/// to drop the request without saying so.
pub fn reset_request_by_address(limiter: &dyn RateLimiter, email: &Email) -> bool {
    limiter.check(
        &RateKey::address(RESET_REQUEST, email),
        RESET_REQUEST_PER_ADDRESS,
    ) == RateDecision::Allowed
}

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
    decide(state.limiter().check(key, limit), correlation_id)
}

fn decide(
    decision: RateDecision,
    correlation_id: audit::CorrelationId,
) -> Result<(), IdentityError> {
    match decision {
        RateDecision::Allowed => Ok(()),
        RateDecision::Limited { retry_after } => Err(IdentityError::new(
            ErrorKind::RateLimited {
                retry_after_secs: u64::try_from(retry_after.whole_seconds().max(1)).unwrap_or(1),
            },
            correlation_id,
        )),
    }
}
