//! A typed, append-only audit journal.
//!
//! Consumable by two or more modules without either depending on the
//! other -- this crate's own `Cargo.toml` names none of them, which is the
//! checkable proof of that claim, mirroring the billing module's own
//! `domain` boundary check. No I/O here: an adapter crate (`audit-pg`, or
//! whatever storage a host chooses) implements [`AuditSink`] against this
//! crate's types.
//!
//! # What this crate deliberately does not have
//!
//! There is no free-form field anywhere in [`AuditEntry`] -- no `String`,
//! no `HashMap<String, String>`, no `details: Option<String>`. A secret, a
//! password or a token has nowhere in this model to go; that is a property
//! of the types, checkable by reading this crate, not a rule enforced by
//! review. For example, [`Actor::User`] takes a [`SubjectId`], not a raw
//! string:
//!
//! ```compile_fail
//! use audit::Actor;
//!
//! // Does not compile: `Actor::User` has no variant that accepts a raw
//! // string. There is no way to attach one to who acted.
//! let _ = Actor::User("not-a-subject-id".to_string());
//! ```
//!
//! There is also no update or delete path: [`AuditSink`] has exactly one
//! method.

mod action;
mod actor;
mod correlation;
mod entry;
mod sink;
mod target;
mod tenant;

pub use action::Action;
pub use actor::{Actor, SubjectId};
pub use correlation::CorrelationId;
pub use entry::AuditEntry;
pub use sink::{AuditError, AuditSink};
pub use target::{Target, TargetId};
pub use tenant::TenantId;
