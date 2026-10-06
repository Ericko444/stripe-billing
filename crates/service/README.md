# tenantry-service

The billing use cases, orchestrating `tenantry-domain`'s ports.

```toml
[dependencies]
tenantry-service = "0.1"
```

## What a consumer gets

`Reads` and `Writes` — the tenant-scoped use cases behind the HTTP routes —
and `WebhookProcessor`, which handles every Stripe event type the module
understands.

## Performs no I/O of its own

Every effect goes through a `tenantry-domain` port, which is why this
crate's own test suite runs with Docker off. Two webhook rules are worth
knowing before you wire it up, because they are easy to assume backwards:

- the local mirror is written **before** your `BillingEventSink` is called —
  the mirror is what Stripe said, the sink is a side effect of it;
- an event is marked processed **only once both** the mirror write and the
  sink call succeed, so a sink failure leaves it pending for a later sweep.

Tenant is always resolved from the database (`stripe_customer_id` → a
`Customer` → its `TenantId`), never from the webhook payload.

Part of [stripe-billing](https://github.com/Ericko444/stripe-billing).
Dual-licensed under MIT or Apache-2.0.
