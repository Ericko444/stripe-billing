# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Two standalone modules — billing / subscriptions and identity — in a Rust
workspace (Axum), plus a React/TS demo host that composes them.

Current state:
- **Billing: complete.** Thirteen migrations; eleven tenant-scoped routes plus
  the Stripe webhook.
- **Audit journal (Phase 7): complete.** One migration, shared by both modules.
- **Identity (Phase 8): complete as planned.** Users in several tenants with
  one role per membership, password login with an `HttpOnly` session cookie,
  tenant selection, password change, password reset, and a minimal members
  slice (list, invite, suspend) — eleven routes, one migration.

`demo` serves all twenty-three routes under `/api/v1`. The frontend — React 19
/ TypeScript / Vite, ~2,300 lines under `frontend/src` — drives billing, login,
tenant selection and password reset; members are API-only.

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo test -p domain            # one crate
cargo test -p domain money::    # one test or module by name
cargo test -p domain -- --nocapture
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

`persistence`, `stripe-adapter`, `audit-pg`, `identity-pg` and `demo` tests
bring up a disposable Postgres via `testcontainers` and need a running Docker
daemon. Webhook work uses
`stripe listen --forward-to localhost:PORT/api/v1/webhooks/stripe`.

Running the demo: `cargo run -p demo -- seed` once (needs `SEED_USER_PASSWORD`
and the `SEED_PLAN_*` ids), then `cargo run -p demo`; seeded users are
`alice@example.test` (Owner of Tenant A, Member of B) and `bob@example.test`
(Owner of B). Reset and invitation links are written to the server log by
`demo`'s `LogMailer` — there is no mail provider.

CI (`.github/workflows/ci.yml`) runs on every pushed branch: the same
boundary/fmt/clippy/test gate (including the identity boundary checks below),
the frontend build and tests with the feature-import check, the declared MSRV,
an append-only check on `migrations/`, `crates/audit-pg/migrations/` and
`crates/identity-pg/migrations/`, and dependency advisories. Warnings fail it
(`-D warnings`).

## Architecture

Twelve crates under `crates/`. Six form the billing module, with **strictly
inward** dependencies:

```
demo ──> api ──> service ──> domain <── persistence
  └────> stripe-adapter ─────────────────────┘
```

- `domain` — models, value objects, ports (traits), error taxonomy. Declarative, no I/O.
- `service` — use cases orchestrating the ports. No I/O of its own.
- `persistence` — sqlx adapters implementing the domain ports.
- `stripe-adapter` — Stripe adapter, idempotency ledger, webhook verification/parsing.
  Named `stripe-adapter`, not `stripe`, because `async-stripe`'s lib name *is*
  `stripe` — this keeps `stripe::` meaning the SDK inside the adapter.
- `api` — Axum **router factory**, wire DTOs, error → HTTP mapping.
- `demo` — the only crate that picks concrete implementations and owns a `main`.

Two are the audit journal (Phase 7; argued in README §5, "Audit trail —
append-only, and why soft delete does not apply here"), free-standing and
outside that chain on purpose:

- `audit` — typed, append-only event model and the `AuditSink` port. No I/O,
  and no dependency on `domain` or any other workspace crate: the identity
  module depends on it too, without either module depending on the other.
- `audit-pg` — Postgres adapter for `audit`: its own schema (`audit`), its own
  migrator (tracked in `audit._sqlx_migrations`, not the billing module's
  `_sqlx_migrations`, so the two migration sets can run against one database
  without colliding), and the append-only grant.

The last four are the identity module (Phase 8; argued in README §6, "The
identity module"), the same inward shape beside billing:

```
demo ──> identity-api ──> identity-service ──> identity-domain <── identity-pg
```

- `identity-domain` — users, memberships, `Role`, sessions, `SplitToken`,
  password types, ports. No I/O.
- `identity-service` — use cases (login, sessions, tenant selection, password
  change and reset, members), plus the Argon2id hasher, OS-random tokens, the
  clock, the in-memory rate limiter and the reset worker.
- `identity-pg` — its own schema (`identity`) and migrator
  (`identity._sqlx_migrations`); repositories write audit rows with
  `audit_pg::insert` in the same transaction, fanning account-level events out
  to one row per active membership.
- `identity-api` — Axum router factory, the `__Host-session` cookie, the
  `AuthenticatedSession` extractor (generic over any router state), the CSRF
  origin check, problem+json.

The modules meet only in `demo/src/identity_tenant.rs`: `IdentityTenant` wraps
`AuthenticatedSession` to satisfy billing's `TenantExtractor`. `crates/api` did
not change for it, and should not.

These boundaries carry the whole design; all are load-bearing in the defense:

- **`domain/Cargo.toml` must never gain `sqlx`, a Stripe client, or `axum`.**
  That dependency list is the checkable proof of the separation.
- **`api` exposes a `Router`, never a binary.** If it owned `main`,
  `tokio::main` or config loading it would not be pluggable into a host that
  already has them.
- **`audit/Cargo.toml` must never gain `domain`, `service`, `persistence`,
  `stripe-adapter`, `api`, `axum`, a Stripe client, or `sqlx`.** That
  dependency list is the checkable proof that two modules can share this
  crate without depending on each other. `audit-pg` is exempt — it is
  expected to depend on `audit` and on `sqlx`.
- **No `identity-*` crate names a billing crate (`domain`, `service`,
  `persistence`, `stripe-adapter`, `api`), and no billing crate names an
  `identity-*` crate.** Only `demo` names both. Checked in CI in both
  directions, by manifest grep and by `cargo tree`.
- **`identity-domain/Cargo.toml` must never gain `sqlx`, `axum` or `argon2`.**
  The identity module's equivalent of the `domain` rule.
- **`frontend/src/identity` and `frontend/src/billing` never import each
  other.** `host/` composes them. Checked in CI.
