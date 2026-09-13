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
pub use facade::Authentication;
pub use session::{ActiveSession, Me, SessionError};
pub use token::{TokenGenerationError, generate_token};
