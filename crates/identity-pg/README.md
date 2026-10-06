# tenantry-identity-pg

Postgres adapter for the identity module: the `identity` schema, its
migrator, and the repositories implementing
`tenantry-identity-domain`'s ports.

```toml
[dependencies]
tenantry-identity-pg = "0.1"
```

## Audit rows commit with the write they describe

Every write that has an audit entry writes it with
`tenantry_audit_pg::insert` **on the same transaction**, so the business
change and the record of it commit or fail together. Account-level events fan
out to one row per active membership, so each tenant sees the events that
concern it.

## It shares a database without coordinating

`run_migrations` owns the `identity` schema and tracks its migrations in
`identity._sqlx_migrations`. The billing module uses the default table and
`tenantry-audit-pg` uses `audit._sqlx_migrations`, so all three run against
one database, in any order, repeatedly. The SQL is embedded at compile time
and travels with the compiled crate.

Part of [stripe-billing](https://github.com/Ericko444/stripe-billing).
Dual-licensed under MIT or Apache-2.0.
