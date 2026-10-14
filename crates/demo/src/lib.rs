//! `demo`'s library surface.
//!
//! `demo` remains the composition root and keeps `main` in `src/main.rs`;
//! this file exists only so `crates/demo/tests/` can reach `demo`'s own
//! modules the way any other crate's integration tests reach its
//! `src/lib.rs`. A bin-only crate has no library target for an integration
//! test to link against. The identity-backed tenant extractor used to live
//! here too; it ships in the `tenantry` facade now, as
//! `tenantry::IdentityTenant`.

pub mod log_mailer;
