# tenantry-audit

A typed, append-only audit journal: the event model and the `AuditSink`
port. No I/O.

```toml
[dependencies]
tenantry-audit = "0.1"
```

## Why it depends on nothing — not even our own crates

This crate's manifest names **no other crate in its workspace**, no HTTP
framework, no database driver and no payment client. CI fails if one
appears.

That is not tidiness, it is the feature. Two independent modules — billing
and identity here — both depend on this crate, and neither depends on the
other. A shared vocabulary crate that reached back into one module would
make the other one's dependency on it a dependency on both. You can adopt it
the same way: depend on `tenantry-audit` from several of your own modules
without coupling them together.

## Secrets have nowhere to go

There is no free-form field anywhere in `AuditEntry` — no `String`, no
`HashMap<String, String>`, no `details: Option<String>`. A password, token
or secret has no place in this model to be put. `Actor::User` takes a
`SubjectId`, not a raw string, so the guarantee is a property of the types
you can check by reading the crate, rather than a convention enforced at
review time.

## Storage

Implement `AuditSink` against whatever you store events in, or take
[`tenantry-audit-pg`](https://crates.io/crates/tenantry-audit-pg) for
Postgres with the append-only grant already in place.

Part of [stripe-billing](https://github.com/Ericko444/stripe-billing).
Dual-licensed under MIT or Apache-2.0.
