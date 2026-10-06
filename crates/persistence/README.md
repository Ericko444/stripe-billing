# tenantry-persistence

Postgres (sqlx) adapters implementing `tenantry-domain`'s repository ports.

```toml
[dependencies]
tenantry-persistence = "0.1"
```

## What a consumer gets

A `Pg*Repository` for each port, and `run_migrations(&PgPool)`.

## The migrations travel with the crate

`run_migrations` applies thirteen numbered, idempotent migrations embedded
into the binary at compile time by `sqlx::migrate!`. They are **not** read
from disk when it runs, so it works from a registry install and from a
binary built on CI and copied into a container with no source tree beside
it. Calling it repeatedly is a no-op.

Applied migrations are tracked in the default `_sqlx_migrations`. If you
also use `tenantry-audit-pg` or `tenantry-identity-pg`, each of those
tracks its own in its own schema, so all three run against one database in
any order.

Every tenant-scoped query carries its `tenant_id` in the `WHERE` clause;
there is no path that reads another tenant's row.

Part of [stripe-billing](https://github.com/Ericko444/stripe-billing).
Dual-licensed under MIT or Apache-2.0.
