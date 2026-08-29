# stripe-billing

A standalone billing / subscriptions module: Rust workspace (Axum) + a React/TS
demo host.

**It is a component, not an application.** The module owns billing and nothing
else — no authentication, no user management, no routing, no UI shell. Anything
a host product would already have, the module consumes rather than provides.

## Layout

```
Cargo.toml            workspace manifest, shared deps in [workspace.dependencies]
crates/
  domain/          models, value objects, ports (traits), error taxonomy
  service/         use cases orchestrating ports — no I/O of its own
  persistence/     sqlx adapters
  stripe-adapter/  Stripe adapter + webhook verification/parsing
  api/             Axum router factory, DTOs, error → HTTP mapping
  demo/            binary: composition root, config, demo-only auth
frontend/             React + TypeScript + Vite (demo host)
migrations/           numbered, idempotent SQL migrations
```

**Dependency direction is strictly inward.** `domain` depends on nothing but
serde/thiserror/uuid/time. `service` depends only on `domain`. `persistence` and
`stripe-adapter` depend on `domain` and implement its traits. `api` depends on `service`
and `domain`. `demo` depends on everything and is the only place where concrete
implementations are chosen.

Two boundaries worth stating, because they are the ones that get blurred:

- `domain`'s `Cargo.toml` has no `sqlx`, no Stripe client and no `axum`.
  That list is the checkable proof of the separation.
- `api` exposes a `Router`, not a binary. If it owned `main`, `tokio::main`
  and config loading, it would not be pluggable into a host that already has them.

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo test -p domain            # one crate
cargo test -p domain money::    # one test or module by name
cargo clippy --workspace --all-targets
cargo fmt --all
cargo doc --workspace --no-deps --open
```

Database tests bring up a disposable Postgres via `testcontainers` and need a
running Docker daemon. Webhooks can be exercised locally with
`stripe listen --forward-to localhost:PORT/webhooks/stripe`.

## Status

Workspace skeleton only — phase 1 of §18. The crate boundaries, dependency
direction, lint policy are in place; the domain model,
adapters, API and frontend are not implemented yet.
