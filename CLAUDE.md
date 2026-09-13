# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A standalone billing / subscriptions module — Rust workspace (Axum) plus a React/TS
demo host.

Current state: **backend complete.** The domain model, both
adapters, the service layer, the Axum router and the demo composition root all
exist and are tested — thirteen migrations, thirteen live routes (eleven
tenant-scoped, plus the Stripe webhook and the demo token mint), served under
`/api/v1`. The frontend is merged — a React 19 / TypeScript / Vite
demo host, ~1,545 lines under `frontend/src`, driving eleven of the thirteen
routes from a browser.

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

`persistence` and `audit-pg` tests bring up a disposable Postgres via
`testcontainers` and need a running Docker daemon. Webhook work uses
`stripe listen --forward-to localhost:PORT/webhooks/stripe`.

CI (`.github/workflows/ci.yml`) runs on every pushed branch: the same
boundary/fmt/clippy/test gate, the frontend build and tests, the declared MSRV,
an append-only check on `migrations/` and `crates/audit-pg/migrations/`, and
dependency advisories. Warnings fail it (`-D warnings`).

## Architecture

Eight crates under `crates/`. Six form the billing module, with **strictly
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

The other two are the audit journal (Phase 7, `docs/plan/phase-7-audit-crate.md`),
free-standing and outside that chain on purpose:

- `audit` — typed, append-only event model and the `AuditSink` port. No I/O,
  and no dependency on `domain` or any other workspace crate: a second,
  not-yet-built module is meant to depend on it too, without either module
  depending on the other.
- `audit-pg` — Postgres adapter for `audit`: its own schema (`audit`), its own
  migrator (tracked in `audit._sqlx_migrations`, not the billing module's
  `_sqlx_migrations`, so the two migration sets can run against one database
  without colliding), and the append-only grant.

Three boundaries carry the whole design; all are load-bearing in the defense:

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

