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

`persistence` tests bring up a disposable Postgres via `testcontainers`
and need a running Docker daemon. Webhook work uses
`stripe listen --forward-to localhost:PORT/webhooks/stripe`.

CI (`.github/workflows/ci.yml`) runs on every pushed branch: the same
boundary/fmt/clippy/test gate, the frontend build and tests, the declared MSRV,
an append-only check on `migrations/`, and dependency advisories. Warnings
fail it (`-D warnings`).

## Architecture

Six crates under `crates/`, with **strictly inward** dependencies:

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

Two boundaries carry the whole design; both are load-bearing in the defense:

- **`domain/Cargo.toml` must never gain `sqlx`, a Stripe client, or `axum`.**
  That dependency list is the checkable proof of the separation.
- **`api` exposes a `Router`, never a binary.** If it owned `main`,
  `tokio::main` or config loading it would not be pluggable into a host that
  already has them.

