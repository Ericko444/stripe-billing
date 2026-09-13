//! The identity module's use cases, and the implementations of the ports
//! that are computation rather than I/O -- Argon2id hashing today.
//!
//! Names no crate from the billing module, and no billing crate names this
//! one; both directions are checked in CI.

mod argon2_hasher;

pub use argon2_hasher::{
    ARGON2_ITERATIONS, ARGON2_MEMORY_KIB, ARGON2_OUTPUT_BYTES, ARGON2_PARALLELISM, Argon2Hasher,
    MAX_CONCURRENT_HASHES,
};
