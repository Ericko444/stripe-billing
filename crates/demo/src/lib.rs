//! `demo`'s library surface.
//!
//! `demo` remains the composition root and keeps `main` in `src/main.rs`
//! unchanged; this file exists only so `crates/demo/tests/` can reach
//! `demo`'s own modules -- starting with the JWT extractor -- the way any
//! other crate's integration tests reach its `src/lib.rs`. A bin-only crate
//! has no library target for an integration test to link against.

pub mod jwt;
pub mod token;
