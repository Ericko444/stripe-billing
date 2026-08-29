# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A standalone billing / subscriptions module — Rust workspace (Axum) plus a React/TS
demo host.

Current state: **workspace skeleton only**. Crate boundaries,
dependency direction, lint policy and migration numbering exist; the domain
model, adapters, API and frontend do not.

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo test -p domain            # one crate
cargo test -p domain money::    # one test or module by name
cargo test -p domain -- --nocapture
cargo clippy --workspace --all-targets
cargo fmt --all
```

`persistence` tests bring up a disposable Postgres via `testcontainers`
and need a running Docker daemon. Webhook work uses
`stripe listen --forward-to localhost:PORT/webhooks/stripe`.

## Architecture

Six crates under `crates/`, with **strictly inward** dependencies:

```
demo ──> api ──> service ──> domain <── persistence
  └────> stripe-adapter ─────────────────────┘
```

- `domain` — models, value objects, ports (traits), error taxonomy. Declarative, no I/O.
- `service` — use cases orchestrating the ports (§8.2). No I/O of its own.
- `persistence` — sqlx adapters implementing the domain ports.
- `stripe-adapter` — Stripe adapter, idempotency ledger, webhook verification/parsing.
  Named `stripe-adapter`, not `stripe`, because `async-stripe`'s lib name *is*
  `stripe` — this keeps `stripe::` meaning the SDK inside the adapter.
- `api` — Axum **router factory**, wire DTOs, error → HTTP mapping.
- `demo` — the only crate that picks concrete implementations and owns a `main`.

Two boundaries carry the whole design; both are load-bearing in the defense:

- **`domain/Cargo.toml` must never gain `sqlx`, a Stripe client, or `axum`.**
  That dependency list is the checkable proof of the separation (S1).
- **`api` exposes a `Router`, never a binary.** If it owned `main`,
  `tokio::main` or config loading it would not be pluggable into a host that
  already has them (§4).

