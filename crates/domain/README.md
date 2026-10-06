# tenantry-domain

Billing and subscription models, value objects, ports and the error
taxonomy. Declarative — **no I/O at all**.

```toml
[dependencies]
tenantry-domain = "0.1"
```

## What a consumer gets

Types (`Customer`, `Plan`, `Subscription`, `Invoice`, `PaymentMethod`,
`Money`, `Currency`, `TenantId`) and the traits the rest of the module is
written against: `SubscriptionRepository`, `BillingProvider`,
`WebhookVerifier`, `BillingEventSink`. `DomainError` is the single error
taxonomy every layer above maps from.

## Why it depends on nothing

Its manifest names no database driver, no HTTP framework and no Stripe
client, and CI fails if one ever appears. That list is the checkable proof
that the models are separable from the machinery — you can implement these
ports against your own storage and your own payment provider without
compiling either of ours.

Money is minor units plus a currency, never a float.

Part of [stripe-billing](https://github.com/Ericko444/stripe-billing).
Dual-licensed under MIT or Apache-2.0.
