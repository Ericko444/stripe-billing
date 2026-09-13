//! The identity module's use cases, and the implementations of the ports
//! that are computation rather than I/O -- Argon2id hashing, OS-random
//! tokens, the system clock.
//!
//! Names no crate from the billing module, and no billing crate names this
//! one; both directions are checked in CI.

mod argon2_hasher;
mod auth;
mod clock;
mod facade;
mod members;
mod password_change;
mod profile;
mod rate_limit;
mod reset;
mod reset_worker;
mod session;
#[cfg(test)]
mod test_support;
mod token;

pub use argon2_hasher::{
    ARGON2_ITERATIONS, ARGON2_MEMORY_KIB, ARGON2_OUTPUT_BYTES, ARGON2_PARALLELISM, Argon2Hasher,
    MAX_CONCURRENT_HASHES,
};
pub use auth::{
    AuthService, LoginError, LoginOutcome, SessionScope, TENANT_SESSION_LIFETIME,
    UNSCOPED_SESSION_LIFETIME,
};
pub use clock::SystemClock;
pub use facade::{Authentication, Members, PasswordResets};
pub use members::{MembersError, MembersService};
pub use password_change::PasswordChangeError;
pub use profile::ProfileError;
pub use rate_limit::InMemoryRateLimiter;
pub use reset::{
    CompleteResetError, INVITATION_TOKEN_LIFETIME, IssueOutcome, PasswordResetService,
    RESET_TOKEN_LIFETIME, ResetError,
};
pub use reset_worker::{
    RESET_QUEUE_CAPACITY, ResetJob, ResetQueue, ResetReceiver, reset_queue, run_reset_worker,
};
pub use session::{ActiveSession, Me, SelectTenantError, SessionError, TenantSelection};
pub use token::{TokenGenerationError, generate_token};
