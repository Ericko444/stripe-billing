# tenantry-audit-pg

Postgres adapter for [`tenantry-audit`](https://crates.io/crates/tenantry-audit):
its own schema, its own migrator, the append-only grant, `insert` and
`PgAuditSink`.

```toml
[dependencies]
tenantry-audit-pg = "0.1"
```

## Two ways to write, for two different callers

- **`insert`** is the primitive you compose into your own transaction, so a
  business write and the record of it commit or fail together. This is the
  one to reach for.
- **`PgAuditSink`** implements `tenantry_audit::AuditSink` over a `PgPool`
  and acquires its own connection per call — for the callers that have no
  surrounding transaction to join.

## Append-only is enforced by the grant, not by the code

The role the application connects as is given `INSERT` and `SELECT` on
`audit.audit_log` and nothing else. `UPDATE` and `DELETE` fail at the
database, so "append-only" does not depend on every future code path
remembering to be careful.

## It shares a database without coordinating

`run_migrations` owns the `audit` schema and tracks its migrations in
`audit._sqlx_migrations`, not the default table. That is what lets it run
against the same database as `tenantry-persistence` (and
`tenantry-identity-pg`) in any order, repeatedly, without the two migration
sets colliding on version numbers. The SQL is embedded at compile time, so
it travels with the compiled crate.

Part of [stripe-billing](https://github.com/Ericko444/stripe-billing).
Dual-licensed under MIT or Apache-2.0.
