# tenantry-identity-api

The identity module's HTTP surface: an Axum router factory, the session
cookie, the session extractor, the CSRF origin check, request correlation
ids and `application/problem+json` errors.

```toml
[dependencies]
tenantry-identity-api = "0.1"
```

## A `Router`, never a binary

`identity_router(state)` mounts the fourteen routes. Like
`tenantry-api`, there is no `main` here — a host mounts this beside whatever
else it serves.

Unlike `tenantry-api`, this crate **does** authenticate. That is the
module's job.

## What you get for authorizing your own routes

`AuthenticatedSession` is an extractor generic over any router state, so you
can require a logged-in session on routes that have nothing to do with this
crate. The session itself is an `HttpOnly` `__Host-session` cookie.

## Composing with the billing module

This crate names no billing crate. The seam is one newtype in the host:
wrap `AuthenticatedSession` in a type that satisfies billing's
`TenantExtractor`, and `tenantry-api` needs no change at all. The worked
example is in the repository at `crates/demo/src/identity_tenant.rs`.

Part of [stripe-billing](https://github.com/Ericko444/stripe-billing).
Dual-licensed under MIT or Apache-2.0.
