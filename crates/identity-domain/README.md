# tenantry-identity-domain

The identity module's domain: users belonging to several tenants, one role
per membership, password authentication and password reset. Declarative, no
I/O.

```toml
[dependencies]
tenantry-identity-domain = "0.1"
```

## Secrets are types, not conventions

A password never travels as a `String`. `Password` wraps a `SecretString`:
its `Debug` is redacted, and it has no `Display` and no `Serialize`, so it
cannot be formatted into a log line or written into a response body by
accident.

`NewPassword` is the only thing a hasher accepts, and the only way to obtain
one is through the policy check — so a password that was never checked
cannot be stored. That is enforced by the type, not by remembering to call
something.

## It knows nothing about billing

This crate names no crate from the billing module, and no billing crate
names it. Both directions are checked in CI. A host composes the two; neither
module learns the other exists.

Part of [stripe-billing](https://github.com/Ericko444/stripe-billing).
Dual-licensed under MIT or Apache-2.0.
