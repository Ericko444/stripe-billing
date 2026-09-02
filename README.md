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

### Running the demo

`cargo run -p demo` reads and validates seven environment variables at
startup (all required; a missing one exits non-zero with a message naming
it):

| Variable | Purpose |
|----------|---------|
| `DATABASE_URL` | Postgres connection string for the mirror and ledger tables |
| `STRIPE_WEBHOOK_SIGNING_SECRET` | the `whsec_…` printed by `stripe listen`; wrapped in `SecretString` on read |
| `STRIPE_SECRET_KEY` | the `sk_…` secret API key the write path's `BillingProvider` uses; wrapped in `SecretString` on read |
| `CHECKOUT_SUCCESS_URL` | where Stripe returns the customer after a completed Checkout Session; host config, never a request field |
| `CHECKOUT_CANCEL_URL` | where Stripe returns the customer if they abandon Checkout |
| `PORT` | TCP port the server binds |
| `BILLING_JWT_SECRET` | HS256 secret `demo`'s own `JwtDecoder`/`POST /demo/token` sign and verify tokens with; `api` has no use for it (Phase 4d, D1) |

`RUST_LOG` tunes tracing output. A `docs/walkthrough/phase-4a-webhook.md`
records an end-to-end run against real Stripe traffic; a full authenticated
walkthrough of every route is in
`docs/walkthrough/phase-4d-verification.md`.

#### Seeding two demo tenants

`cargo run -p demo -- seed` creates two fixed-id tenants to switch between,
each with a Stripe customer, two local plans, and one subscription. It needs
four more variables, read only by the seed itself (never required to serve):

| Variable | Purpose |
|----------|---------|
| `SEED_PLAN_A_STRIPE_PRICE_ID` / `SEED_PLAN_A_STRIPE_PRODUCT_ID` | the first plan's Stripe price/product ids |
| `SEED_PLAN_B_STRIPE_PRICE_ID` / `SEED_PLAN_B_STRIPE_PRODUCT_ID` | the second plan's |

Point these at any two prices in your own Stripe test-mode account — the
seed doesn't create or verify them, only mirrors them locally, so a wrong id
here is a display-only mistake, never a broken write path. Re-running the
seed is a no-op: it finds the same two tenants and creates nothing new.

**Both subscriptions seed `incomplete`, not `active` — this is expected.**
`BillingProvider` has no way to attach a payment method (that has only ever
happened through Stripe's own hosted UI or a confirmed SetupIntent), so a
subscription created for a customer with no card stays `incomplete` until
one is attached. Call `POST /payment-methods/setup-intent` and confirm it
with a test card, or run the Checkout Session flow, to watch a seeded
subscription go `active` — that write path, the webhook it triggers, and the
mirror update are the thing this module actually demonstrates. See
`docs/walkthrough/phase-4d-verification.md` for a full run.

## Routes

`api::billing_router` mounts eleven tenant-scoped routes; `api::webhook_router`
serves `POST /webhooks/stripe` alone (no tenant). `billing_router` is generic
over a host-supplied tenant extractor (`TenantExtractor`) — the tenant never
crosses the wire as input, and ids in a path or body are always **local**
uuids, resolved to Stripe ids server-side.

**Read (Phase 4b)** — local mirror only, no outbound Stripe call:

| Method | Path | Returns |
|--------|------|---------|
| GET | `/plans` | the tenant's plans |
| GET | `/subscription` | the current subscription, or `null` (200, not 404) |
| GET | `/invoices` | keyset-paginated page (`?limit=` 1–100, `?after=` opaque cursor) |
| GET | `/invoices/{id}` | one invoice; unknown id and another tenant's id both 404 |
| GET | `/payment-methods` | the tenant's stored cards (brand / last4 / default only) |

**Write (Phase 4c)** — each calls Stripe through the idempotency ledger;
ownership is checked before the outbound call, so a cross-tenant id is a 404
with no call made (§7.4):

| Method | Path | Does |
|--------|------|------|
| POST | `/subscriptions/checkout-session` | Checkout Session (`subscription` mode); returns the hosted `url` |
| POST | `/subscriptions/{id}/change-plan` | change plan, prorated; snapshot applied through the §10.2 guard |
| POST | `/subscriptions/{id}/cancel` | cancel at period end (`at_period_end` absent ⇒ `true`) or immediately |
| POST | `/payment-methods/setup-intent` | SetupIntent → `client_secret` for the frontend |
| POST | `/payment-methods/{id}/default` | set default (Stripe first, then the mirror in one statement) |
| DELETE | `/payment-methods/{id}` | detach at Stripe, then soft-delete the mirror; 204 |

**Webhook (Phase 4a):**

| Method | Path | Does |
|--------|------|------|
| POST | `/webhooks/stripe` | verify signature, dedup, apply to the mirror; no tenant context |

**Demo scaffolding (Phase 4d), `demo` only:**

| Method | Path | Does |
|--------|------|------|
| POST | `/demo/token` | mints a token for any `tenant_id` given to it — **not authentication**, no identity check of any kind. Scaffolding so a reviewer without a real host's login flow can still drive the tenant-scoped routes above; `demo` logs a warning naming this route at every startup |

All thirteen routes are mounted together in `demo`: the eleven tenant-scoped
routes behind `demo`'s own `DemoTenant` (a ~70-line JWT extractor a real host
replaces with its own — see `billing_router`'s rustdoc for the ~20-line
middleware-adapter shape), the webhook route verified by signature instead,
and `/demo/token` with no auth of its own. Exercised end to end — real
Postgres, real Stripe test mode, both seeded tenants, all thirteen routes —
in `docs/walkthrough/phase-4d-verification.md`.

## Status

Phase 4d complete: `demo` mounts the real `billing_router::<DemoTenant>`
alongside `webhook_router` and `POST /demo/token`; `api` gained
`ApiError::Unauthorized` (401, `WWW-Authenticate: Bearer`, one coarse detail
for every cause) and lost the empty `jwt-auth` feature — authentication is
the host's job, and `api` is complete without knowing what fills it (D1).
`cargo run -p demo -- seed` gives the tenant switcher two tenants, each with
a Stripe customer, two plans and one (`incomplete`, by design — see
"Seeding two demo tenants" above) subscription.

Phase 4c delivered the six write routes above, `BillingProvider` expanded
with SetupIntent, Checkout Session and payment-method set-default/detach,
all going through the Phase 2 idempotency ledger; `service`'s `Writes` use
cases enforcing §7.4's orderings; `AppState` carrying the host-supplied
`CheckoutUrls`. Phase 4b delivered the five read routes, the generic tenant
extractor, `api`-owned DTOs, and keyset invoice pagination (migration
`0013`). Phase 4a delivered the `service` webhook processor (every
`init-spec.md` §10.4 event type), the `persistence` adapters, `POST
/webhooks/stripe`, and the `demo` composition root. The frontend is still a
later phase.
