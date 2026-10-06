# tenantry-api

The billing module's HTTP surface: an Axum **router factory**, wire DTOs,
and the error-to-HTTP mapping.

```toml
[dependencies]
tenantry-api = "0.1"
```

## It exposes a `Router`, never a binary

There is no `main` here, no `#[tokio::main]` and no config loading. A host
that already owns those mounts this crate's router into its own server.
That is the whole point: if this crate owned a `main`, it could not be
plugged into a service that already has one.

Two factories:

- `webhook_router(state)` mounts `POST /webhooks/stripe` and nothing else;
- `billing_router::<T>(state)` mounts the eleven tenant-scoped routes.

## What the host must supply: a tenant extractor

`billing_router` is generic over `T: TenantExtractor`. **This crate never
authenticates** — you do, and you hand it a type that turns an
authenticated request into a `TenantId`. `TenantExtractor` is a trait alias
by blanket impl, so there is no `impl` block to write; a type qualifies when
it is an Axum `FromRequestParts<AppState>` extractor that rejects with
`ApiError` and converts `Into<TenantId>`.

Pinning the rejection type is the load-bearing clause: it keeps an
extraction failure rendering as `application/problem+json` like every other
error, instead of letting one route family answer differently from the rest.
Forgetting to wire an extractor is a compile error, not a 500 in production.

If your auth layer already resolves the tenant into request extensions, a
~10-line newtype is enough; the snippet is on `billing_router`'s rustdoc.

## Two deliberate constraints

- **The webhook body is read as raw `Bytes`, never `Json<T>`.** The Stripe
  signature is an HMAC over the exact bytes sent; a `Json<T>` extractor that
  consumes and re-encodes the body breaks verification irrecoverably.
- **DTOs are owned here; domain types are never `Serialize`.** Every
  response body is a type in this crate with a hand-written `From`. The
  moment a domain struct becomes serializable, adding a field to it is a
  wire-breaking change made by someone not thinking about the wire. Money is
  `{amount_minor, currency}`; timestamps are RFC 3339 UTC.

Part of [stripe-billing](https://github.com/Ericko444/stripe-billing).
Dual-licensed under MIT or Apache-2.0.
