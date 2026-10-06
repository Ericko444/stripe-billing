# tenantry-identity-service

The identity module's use cases, plus the implementations of the ports that
are computation rather than I/O.

```toml
[dependencies]
tenantry-identity-service = "0.1"
```

## What a consumer gets

Login, session resolution and rotation, tenant selection, password change,
password reset (request and complete), the members slice (list, invite,
suspend) and account deactivation/reactivation.

And, because they are computation rather than storage: the Argon2id hasher,
OS-random token generation, the system clock, an in-memory rate limiter and
the reset worker. Those are behind `tenantry-identity-domain` ports too, so
you can substitute your own.

## It knows nothing about billing

Names no crate from the billing module, and no billing crate names it. Both
directions are checked in CI.

Part of [stripe-billing](https://github.com/Ericko444/stripe-billing).
Dual-licensed under MIT or Apache-2.0.
