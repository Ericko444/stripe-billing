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
//!
//! # Why append-only suspends this workspace's soft-delete pattern
//!
//! Every other table in the billing module carries `deleted_at`: a row
//! models *current* state, which a tenant can retract (cancel a
//! subscription, detach a card), so hiding it while keeping it for
//! referential history is the right shape. `audit_log` has no such column,
//! deliberately -- an audit entry does not model current state at all. It
//! models a fact about what happened, at a specific instant, and a fact
//! does not become untrue later. There is nothing to retract.
//!
//! A soft delete is still an `UPDATE`. Reusing the pattern here would mean
//! granting the one privilege -- write access to an existing row -- that
//! would let a tampered-with entry look identical to a real one. The value
//! of an audit log is exactly that this cannot happen, so the Postgres role
//! the application writes as (`audit_writer`, granted in `audit-pg`'s
//! `migrations/0001_audit_log.sql`) is given `INSERT` and `SELECT` only. No
//! `UPDATE`, no `DELETE`, on the table or the role.
//!
//! **The erasure tension, named rather than hidden:** a data-subject
//! erasure request (a tenant offboarding, a "delete my account" flow) is in
//! real tension with "never deletable." The answer is not an exception to
//! append-only; it is that [`AuditEntry`] was never a place personal data
//! could reach. Every identifying field is an opaque id -- [`TenantId`],
//! [`SubjectId`], [`TargetId`] -- never a name, an email, or free text (see
//! the section above: there is no field for one). Erasing a person means
//! erasing the row that resolves `SubjectId` to a name in whatever module
//! owns identity; the audit row survives, but it was never personal data to
//! begin with, only a pointer to a record that may no longer resolve to
//! anything. The log stays honest about *that something happened*, which is
//! its whole job, without being a second place a right-to-erasure request
//! has to reach.

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
