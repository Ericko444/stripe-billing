//! One dependency line for the `tenantry-*` crates: multi-tenant billing on
//! Stripe, an identity module, and the audit journal they share, each behind
//! a Cargo feature.
//!
//! ```toml
//! [dependencies]
//! tenantry = { version = "0.1", features = ["full"] }
//! ```
//!
//! Nothing is enabled by default. Each underlying crate is re-exported whole,
//! as a module, rather than glob-flattened into this one: several names exist
//! in more than one crate as distinct types (`TenantId` in billing, identity
//! and the audit journal; `CorrelationId` in the audit journal and billing's
//! API), and Rust drops an ambiguous glob re-export silently instead of
//! failing. The paths also keep the module boundary visible --
//! `tenantry::billing` and `tenantry::identity` never appear in each other's
//! signatures.
//!
//! The underlying crates stay published and usable directly; this crate only
//! adds a front door.
//!
//! # Features
//!
//! `audit`, `billing`, `identity` and `stripe`, plus one Postgres feature per
//! module (`audit-postgres`, `billing-postgres`, `identity-postgres`) and
//! `full` for all of them. `billing` compiles no identity crate and no
//! `sqlx`; `identity` compiles no billing crate and no Stripe client. With
//! both `billing` and `identity` on, [`IdentityTenant`] joins them.
//!
//! # What you bring yourself
//!
//! - A `Mailer`. None that sends mail ships here, and without one password
//!   reset and invitations do not deliver.
//! - A shared `RateLimiter` if you run more than one process: the shipped
//!   `InMemoryRateLimiter` counts per process.
//! - `axum` 0.8 and `sqlx` 0.9 in your own manifest. No third-party crate is
//!   re-exported.
//!
//! `examples/host.rs` in this crate's package is the whole composition,
//! compiled under `full`. What the seam between the modules costs is on
//! [`IdentityTenant`].

// Feature badges on docs.rs, which builds on nightly with `--cfg docsrs`.
// Stable never sets `docsrs`, so it never sees the nightly feature.
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(all(feature = "billing", feature = "identity"))]
mod identity_tenant;
#[cfg(all(feature = "billing", feature = "identity"))]
pub use identity_tenant::IdentityTenant;

/// The audit journal: the typed, append-only event model and the
/// `AuditSink` port. Shared by both modules, depending on neither.
#[cfg(feature = "audit")]
#[doc(inline)]
pub use tenantry_audit as audit;

/// The audit journal's Postgres adapter: its schema, migrator and sink.
#[cfg(feature = "audit-postgres")]
#[doc(inline)]
pub use tenantry_audit_pg as audit_postgres;

/// The billing module: models and ports, use cases, and the Axum router,
/// plus the Stripe and Postgres adapters behind their own features.
#[cfg(feature = "billing")]
pub mod billing {
    #[doc(inline)]
    pub use tenantry_api as api;
    #[doc(inline)]
    pub use tenantry_domain as domain;
    #[doc(inline)]
    pub use tenantry_service as service;

    /// The Stripe adapter. Needs the `stripe` feature.
    #[cfg(feature = "stripe")]
    #[doc(inline)]
    pub use tenantry_stripe_adapter as stripe;

    /// The Postgres repositories and billing's migrator. Needs the
    /// `billing-postgres` feature.
    #[cfg(feature = "billing-postgres")]
    #[doc(inline)]
    pub use tenantry_persistence as postgres;
}

/// The identity module: users, memberships and sessions, the use cases, and
/// the Axum router, plus the Postgres adapter behind its own feature.
#[cfg(feature = "identity")]
pub mod identity {
    #[doc(inline)]
    pub use tenantry_identity_api as api;
    #[doc(inline)]
    pub use tenantry_identity_domain as domain;
    #[doc(inline)]
    pub use tenantry_identity_service as service;

    /// The Postgres repositories and identity's migrator. Needs the
    /// `identity-postgres` feature.
    #[cfg(feature = "identity-postgres")]
    #[doc(inline)]
    pub use tenantry_identity_pg as postgres;
}
