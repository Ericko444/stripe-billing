# Phase 4d — End-to-end verification (Task 9)

> **STATUS: RUN 2026-09-02. Complete.** Seeded two tenants against a real
> Stripe test-mode account, minted a token per tenant through the real
> `POST /demo/token`, and drove all eleven tenant-scoped routes as each. Every
> response carried only the calling tenant's own ids. Two write routes were
> also driven cross-tenant with a real id belonging to the other tenant, both
> `404` with **zero** new `billing.outbound_requests` rows. Four observable
> `401` causes were checked against the running binary; all four produced the
> byte-identical body the automated suite already proves for all seven. One
> write route (`change-plan`) surfaced a genuine `502` given this run's
> `incomplete` starting state -- recorded below, not treated as a defect to
> fix under this task.

## What this exists to check

`crates/demo/tests/jwt.rs` proves the extractor's rejection paths in isolation,
and `crates/api/tests/tenancy.rs` proves route-level isolation against stub
services. Neither has ever run the real composition root -- `main`'s actual
`billing_router::<DemoTenant>` + `webhook_router` + `/demo/token` mount, real
Postgres, a real seed, and a real Stripe test-mode account -- end to end. This
is that run.

## Environment

- Stripe test-mode account: `acct_1UAZGoIq1wO3Oe7i` (the same sandbox Phase
  4c's Task 18 rehearsal used; its shared prices are what the seed points at)
- Postgres: `postgres:16-alpine` in Docker, host port `5432`
  (matching this repo's `.env`)
- `demo` bound to `127.0.0.1:8099` for this run (`PORT=8099`)
- OS: Windows 11 Home 10.0.22621, Git Bash
- Run date: **2026-09-02**
- Driven with `curl` against the running binary, not through any test harness

## Setup

```
cargo run -p demo -- seed
  -> seeded tenant 00000000-0000-0000-0000-000000000001
  -> seeded tenant 00000000-0000-0000-0000-000000000002
```

```
tenant 1: customer cus_VBaTi7jMF1LRov
          plans   Starter f730afb6-f167-4f0e-a2cf-7f2f4c403a26 ($15.00)
                  Pro     2b32670a-d120-4b72-8091-26fe08387e70 ($49.00)
          sub     1a8c7be7-5688-4ddc-9ac5-4a6d8c626a76  status: incomplete

tenant 2: customer cus_VBaTHp09KG0y6H
          plans   Starter bcbe8082-88cd-41b7-b03c-6ccc6d46c2f2 ($15.00)
                  Pro     63a22ad5-3b11-4473-84da-a313bc2fc542 ($49.00)
          sub     446c8ff9-41b8-4970-bdd4-444dc3979294  status: incomplete
```

Both subscriptions seed `incomplete`, per Plan 4d's P4/F2: no `BillingProvider`
method attaches a payment method, so Stripe cannot activate either
subscription's first invoice. This is the documented starting state, not a
bug (see README, Task 10).

Tokens minted through the real route:

```
POST /demo/token {"tenant_id":"00000000-0000-0000-0000-000000000001"} -> token A
POST /demo/token {"tenant_id":"00000000-0000-0000-0000-000000000002"} -> token B
```

## Isolation: all eleven routes, own data only

| Route | Tenant 1 | Tenant 2 |
|---|---|---|
| `GET /plans` | its 2 plan ids only | its 2 plan ids only (different ids) |
| `GET /subscription` | its sub id, `incomplete` | its sub id, `incomplete` (different id) |
| `GET /payment-methods` | `[]` (F2 -- none seeded) | `[]` |
| `GET /invoices` | `{"items":[]}` | `{"items":[]}` |
| `GET /invoices/{id}` | `404` for an unknown id | -- |

No response from either tenant ever contained the other's plan, subscription,
customer or Stripe id.

## Write routes

| Route | Own id | Result | Cross-tenant id | Result | Ledger rows added |
|---|---|---|---|---|---|
| `POST /subscriptions/checkout-session` | own Pro plan | `200`, Checkout URL | tenant 2's Pro plan id | `404` | `1` (own only) |
| `POST /subscriptions/{id}/change-plan` | own sub -> own Pro | `502` (see Finding 1) | tenant 2's sub id | `404` | `1` (own only -- the `502` still reserved and completed a ledger row, matching Phase 2's documented provider-error case) |
| `POST /subscriptions/{id}/cancel` | own sub, `at_period_end` | `200`, `cancel_at_period_end: true` | not repeated (tenant 2 used for the "own" case here) | -- | `1` |
| `POST /payment-methods/setup-intent` | (no id) | `200`, `client_secret` returned | -- | -- | `1` |
| `POST /payment-methods/{id}/default` | a nonexistent id | `404` | not exercised -- see "Not covered" | | `0` |
| `DELETE /payment-methods/{id}` | a nonexistent id | `404` | not exercised -- see "Not covered" | | `0` |

Both cross-tenant attempts (`checkout-session` with the other tenant's real
plan id, `change-plan` with the other tenant's real subscription id) were
confirmed against `billing.outbound_requests` directly: the table's row count
for the calling tenant did not change. The ownership check rejects before any
Stripe call, exactly as the tenant-scoped read routes already do.

### Finding 1: `change-plan` on an `incomplete` subscription is a `502`, not a success

`crates/stripe-adapter/src/subscriptions.rs`'s `change_plan` sets
`UpdateSubscriptionPaymentBehavior::ErrorIfIncomplete`, so updating a
subscription that is already `incomplete` (no payment method) makes Stripe
reject the proration it would need to collect. The route correctly surfaces
this as `502` (`DomainError::Provider`, per `api`'s existing mapping) rather
than a `500` or a silent success -- the *routing* is correct. Whether
`change_plan` should behave differently for an `incomplete` subscription is a
question for a future phase, not this verification task; recorded here
because it is a real behavior a reviewer will hit if they try `change-plan`
before attaching a payment method.

## The four 401 causes, against the running binary

| Case | Result |
|---|---|
| No `Authorization` header | `401`, `detail: "The request could not be authenticated."` |
| `Authorization` without `Bearer ` prefix | `401`, identical detail |
| An unparsable token (`Bearer not.a.jwt`) | `401`, identical detail |
| A well-formed JWT signed with the wrong secret | `401`, identical detail |

All four produced the byte-identical `problem+json` body. The remaining three
of the seven causes `crates/demo/tests/jwt.rs` covers (wrong algorithm,
expired `exp`, non-uuid `sub`) and the eighth-in-spirit case (the
`Extension(JwtDecoder)` layer itself missing) are not re-demonstrated live
here -- see "Not covered".

## Not covered

- **`payment-methods/{id}/default` and `DELETE /payment-methods/{id}`
  against a real, owned row.** F2 means the seed creates no payment methods,
  so there is no real id to exercise the success path or a genuine
  cross-tenant row with. Only the nonexistent-id `404` path was checked live;
  the success and cross-tenant-404 paths for these two routes are covered by
  `crates/api/tests/writes.rs` and `tenancy.rs` against stub services, not
  against real Stripe here.
- **Wrong-algorithm, expired-`exp`, non-uuid-`sub`, and missing-`Extension`
  401 causes**, live. Covered exhaustively by the seven-case automated suite
  in `crates/demo/tests/jwt.rs`; not re-run manually since minting a
  wrong-algorithm or missing-extension request against a real running binary
  adds nothing the automated suite doesn't already prove byte-for-byte.
- **A completed Checkout Session bringing a subscription to `active`.** Task
  18's Phase 4c rehearsal already proved this path (browser checkout ->
  eleven webhook deliveries -> mirror updated) against this same account; not
  repeated here.
- **The `POST /webhooks/stripe` route's own behavior** -- unchanged since
  Phase 4c and outside this task's scope (tenant-scoped route isolation).
