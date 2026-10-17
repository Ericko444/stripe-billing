# tenantry

Multi-tenant billing on Stripe, an identity module, and the audit journal
they share, behind one dependency and Cargo features.

```toml
[dependencies]
tenantry = { version = "0.1", features = ["full"] }
# Not re-exported: name them yourself, at compatible versions.
axum = "0.8"
sqlx = { version = "0.9", features = ["postgres", "runtime-tokio"] }
```

## Features

Nothing is on by default. Every feature is additive.

| Feature             | Gives you                                                        | Pulls in                     |
|---------------------|------------------------------------------------------------------|------------------------------|
| `audit`             | `tenantry::audit`: the append-only event model, `AuditSink`      | no I/O                       |
| `billing`           | `tenantry::billing::{domain, service, api}`                      | `audit`, `axum`              |
| `identity`          | `tenantry::identity::{domain, service, api}`                     | `audit`, `axum`              |
| `stripe`            | `tenantry::billing::stripe`: the Stripe adapter                  | `billing`, `async-stripe`    |
| `audit-postgres`    | `tenantry::audit_postgres`: schema, migrator, `PgAuditSink`      | `audit`, `sqlx`              |
| `billing-postgres`  | `tenantry::billing::postgres`: repositories, migrator            | `billing`, `audit-postgres`  |
| `identity-postgres` | `tenantry::identity::postgres`: repositories, migrator           | `identity`, `audit-postgres` |
| `full`              | all of the above                                                 |                              |

With both `billing` and `identity` on, you also get
`tenantry::IdentityTenant`, the seam between them (below).

The modules stay apart: `-F billing` compiles no identity crate and no
`sqlx`, and `-F identity` compiles no billing crate and no Stripe client.
There is no bare `postgres` feature, because it would have to turn on both
modules' adapters.

## The composition

The whole of it is [`examples/host.rs`](examples/host.rs), compiled in CI
(`cargo run -p tenantry --example host --features full`). The part that
joins the two modules:

```rust
use axum::{Router, extract::Extension, middleware};
use tenantry::IdentityTenant;
use tenantry::billing::api::{billing_router, webhook_router};
use tenantry::identity::api::{AllowedOrigins, identity_router, origin_check};

// `billing` is billing's `AppState`, `identity` is identity's `IdentityState`,
// both built from the Postgres adapters, the Stripe adapter and the services.
let api = billing_router::<IdentityTenant>(billing.clone())
    .merge(webhook_router(billing))
    .merge(identity_router(identity.clone()))
    // Lets `IdentityTenant` find a session on billing's routes.
    .layer(Extension(identity))
    // Protects every cookie-authenticated write, billing's included.
    .layer(middleware::from_fn_with_state(
        AllowedOrigins::new(&config.allowed_origins),
        origin_check,
    ));
let app = Router::new().nest("/api/v1", api);

// Identity rate-limits by client address, so serve with connect info.
axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
```

Run the three migrators first, against one database. Each keeps its own
`_sqlx_migrations` table in its own schema, so they do not collide:

```rust
tenantry::billing::postgres::run_migrations(&pool).await?;
tenantry::audit_postgres::run_migrations(&pool).await?;
tenantry::identity::postgres::run_migrations(&pool).await?;
```

## What you bring yourself

- **A `Mailer`.** The library defines the trait
  (`tenantry::identity::domain::Mailer`) and ships **no implementation that
  sends mail**. Without one, password reset and invitations do not deliver.
  The example has a logging one marked not for production. It writes reset
  links to the log, so anyone who can read the log can use them.
- **A shared `RateLimiter` if you run more than one process.** The shipped
  `InMemoryRateLimiter` counts per process, so N processes allow N times
  the login attempts.
- **A `BillingEventSink`.** This is where you provision or revoke features
  when a subscription changes.
- **`axum` 0.8 and `sqlx` 0.9 in your own manifest.** Third-party crates
  are not re-exported.
- Your configuration, your `main` and your runtime. This crate has no
  builder and loads no config.

## What the seam costs

`IdentityTenant` turns an identity session into billing's `TenantId`.
Billing's contract with a host stays one value, a tenant, and keeping it
that way has three costs:

- **No role-based billing authorization.** Any active membership can use
  billing's routes, so a Member can cancel the tenant's subscription.
- **No `403` from billing.** Every refusal (no cookie, an expired session, a
  suspended membership, no tenant chosen yet) becomes billing's one `401`,
  and the response does not say which.
- **Billing's audit rows record `Actor::System`.** Billing's request context
  carries a tenant, not a user.

Each one needs a deliberate change to billing's contract. A workaround in
the host won't do it.

## When to use the underlying crates instead

The eleven `tenantry-*` crates stay published, and this crate only
re-exports them. Depend on them directly if you want one layer without its
module's router, for example `tenantry-domain` alone, or the service layer
behind your own HTTP surface. Do the same if you'd rather pin each crate
yourself, or if you're writing an adapter of your own against a port. The
paths are the same: `tenantry::billing::domain` *is* `tenantry_domain`.

Part of [stripe-billing](https://github.com/Ericko444/stripe-billing).
Dual-licensed under MIT or Apache-2.0.
