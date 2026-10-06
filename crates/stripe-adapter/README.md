# tenantry-stripe-adapter

The Stripe adapter: the outbound `BillingProvider` path, the inbound
webhook path, and the idempotency ledger between them.

```toml
[dependencies]
tenantry-stripe-adapter = "0.1"
```

## What a consumer gets

`StripeBillingProvider` (implements `tenantry_domain::BillingProvider`),
`build_client` with a base-URL override for testing against a mock,
`StripeWebhookVerifier`, the `Ledger` reserve/complete state machine, and
the `StripeError` taxonomy with its flattening to `DomainError`.

## Named `-stripe-adapter`, and the lib path matters

The lib path is `tenantry_stripe_adapter`, never `stripe`, because
`async-stripe`'s own lib name *is* `stripe` — keeping them distinct is what
lets `stripe::` keep meaning the SDK inside this crate.

## The Stripe version requirement is yours to pin

This crate asks for `async-stripe` `1.0.0-rc.8` as a caret range, not an
exact pin. A library that pins a release candidate exactly inflicts that
choice on everyone downstream; if you want the hard pin, write it in your
own manifest. `BillingProvider` is what bounds a breaking SDK change to
this crate.

Ordering against the local mirror is a `tenantry-service` concern, not this
crate's: each method here makes the call and returns a `tenantry-domain`
snapshot.

Part of [stripe-billing](https://github.com/Ericko444/stripe-billing).
Dual-licensed under MIT or Apache-2.0.
