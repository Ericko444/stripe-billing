# stripe-billing

Two standalone modules — **billing / subscriptions** and **identity** — in a
Rust workspace (Axum), plus a React/TypeScript demo host that composes them.

**Each is a pluggable component.** Billing owns billing and nothing else — no
authentication, no user management, no UI shell; it receives a tenant from
whatever host mounts it. Identity owns users, tenant memberships, sessions and
password reset, and has never heard of billing. Neither module's crates name
the other's; the host (`demo`) is the one place they meet, and billing's `api`
crate did not change for it (§6).

---

## 1. What this is

The module mirrors Stripe. Stripe is the authority on what a subscription
costs and whether an invoice was paid; the Postgres tables here are a local
cache of what Stripe last said, kept current by webhooks. Every write goes to
Stripe first and is mirrored afterwards, never the reverse.

What a host supplies, and the module never implements:

- **Authentication and identity.** The module receives a `TenantId` from the
  host through a trait it does not implement. In this repository the host
  satisfies that trait with a session from the identity module (§6).
- **Permissions, routing, and the surrounding UI.**

Out of scope / TODO later:

- **Tax, dunning and revenue recognition.**
- **Stripe's hosted customer portal** — it would handle plan changes and
  payment methods for us, which is the work this module exists to show.
- **Metered / usage-based billing.**

---

## 2. Architecture

```
crates/
  domain/          models, value objects, ports (traits), error taxonomy
  service/         use cases orchestrating ports — no I/O of its own
  persistence/     sqlx adapters implementing the domain ports
  stripe-adapter/  Stripe client, idempotency ledger, webhook verification
  api/             Axum router factory, wire DTOs, error → HTTP mapping
  demo/            binary: composition root, config, seed, the seam between modules
  audit/           typed, append-only audit event model + the AuditSink port
  audit-pg/        Postgres adapter for audit: its own schema and migrator
  identity-domain/  users, memberships, roles, sessions, tokens, ports — no I/O
  identity-service/ use cases, Argon2id hasher, OS-random tokens, rate limiter
  identity-pg/      Postgres adapter for identity: its own schema and migrator
  identity-api/     Axum router factory, session cookie and extractor, CSRF check
frontend/          React + TypeScript + Vite (the demo host)
migrations/        billing's thirteen numbered, idempotent SQL migrations
```

| crate | depends on | kind |
|---|---|---|
| `domain` | *nothing* | pure library |
| `service` | `domain`, `audit` | pure library |
| `persistence` | `domain`, `audit`, `audit-pg` | adapter |
| `stripe-adapter` | `domain` | adapter |
| `api` | `domain`, `service` | library — a `Router` factory |
| `demo` | all of the above | **the only binary** |
| `audit` | *nothing* | pure library, free-standing |
| `audit-pg` | `audit` | adapter, free-standing |
| `identity-domain` | `audit` | pure library |
| `identity-service` | `identity-domain`, `audit` | library + computational adapters |
| `identity-pg` | `identity-domain`, `audit`, `audit-pg` | adapter |
| `identity-api` | `identity-domain`, `identity-service`, `audit` | library — a `Router` factory |

**Dependency direction is strictly inward.** `domain` declares the ports —
`SubscriptionRepository`, `BillingProvider`, `WebhookVerifier`,
`BillingEventSink`. `persistence` and `stripe-adapter` implement them.
`service` orchestrates them without knowing which adapter is behind them.

Two absences carry the design:

- **`api` depends on neither `persistence` nor `stripe-adapter`** — it holds
  `Arc<dyn Reads>`, `Arc<dyn Writes>` and friends, supplied by a host.
- **`service` does not either**, which is why its use cases — including the
  whole webhook processor — test with Docker off against in-memory doubles.

One edge breaks the picture, worth naming before it is found: `stripe-adapter`
lists `persistence` as a **dev-dependency**, because the ledger tests run the
real adapter against a real Postgres. It never appears in a production build.

**`audit` and `audit-pg` sit outside this chain entirely.** They are a
separate, typed audit journal, laid out so a second, not-yet-built module can
write to the same table without either module depending on the other.
`audit/Cargo.toml` must never gain `domain`, `service`, `persistence`,
`stripe-adapter`, `api`, `axum`, or a Stripe client — checked in CI beside the
`domain` boundary job, and it is the checkable proof that neutrality claim
rests on rather than a docstring. `audit-pg` is exempt: it exists to depend on
`audit` and `sqlx`.

**The identity crates are that second module**, with the same inward shape
(`identity-api → identity-service → identity-domain ← identity-pg`) and both
of its writers — identity and billing — sharing `audit` without naming each
other. CI checks it in both directions: no `identity-*` manifest or dependency
tree contains a billing crate, no billing crate's contains an `identity-*`
crate, and `identity-domain` gains neither `sqlx`, `axum` nor `argon2`. The
frontend mirrors it: `src/identity/` and `src/billing/` never import each
other, and `host/` composes both.

## 3. Running it

Four terminals. Postgres and Docker running; the Stripe CLI logged in to the
same test-mode account as `STRIPE_SECRET_KEY`.

```bash
cargo run -p demo -- seed                                          # once
cargo run -p demo
stripe listen --forward-to localhost:8080/api/v1/webhooks/stripe   # note /api/v1
cd frontend && npm install && npm run dev                          # :5173
```

Everything is served under `/api/v1`, webhook included. The Vite dev server
proxies `/api` to `:8080`, so the browser talks to one origin and there is no
CORS layer anywhere in the workspace — CORS is host policy, and keeping it out
of the module is the point.

### Configuration

Eight variables are required, validated at startup; a missing one exits
non-zero naming it. Copy `.env.example` to `.env` (gitignored).

| Variable | Purpose | Secret |
|---|---|---|
| `DATABASE_URL` | Postgres connection for the mirror and ledger tables | yes |
| `STRIPE_SECRET_KEY` | the `sk_…` key the write path uses | **yes** |
| `STRIPE_WEBHOOK_SIGNING_SECRET` | the `whsec_…` printed by `stripe listen` | **yes** |
| `CHECKOUT_SUCCESS_URL` | `http://localhost:5173/checkout/return` — host config, never a request field | no |
| `CHECKOUT_CANCEL_URL` | `http://localhost:5173/` | no |
| `PORT` | TCP port the server binds | no |
| `IDENTITY_ALLOWED_ORIGINS` | origins a cookie-carrying write may come from (CSRF); `http://localhost:5173` | no |
| `IDENTITY_PUBLIC_BASE_URL` | the frontend origin reset and invitation links point at | no |

`RUST_LOG` is optional, and so is `IDENTITY_TRUSTED_PROXY`: set it only
behind a reverse proxy, to that proxy's address, and `X-Forwarded-For` is
believed from it and nothing else (the rate limiter keys on client IP). Secrets become `SecretString` at the moment of
reading, so a stray `#[derive(Debug)]` cannot put a Stripe key in a log line,
and none is ever placed in a URL. The Stripe **API version is pinned at
compile time** from the SDK, not configured — a bump is a `Cargo.toml` change
rather than a second place that can drift.

One variable reaches the browser: `VITE_STRIPE_PUBLISHABLE_KEY`. Vite inlines
*every* `VITE_`-prefixed variable into the bundle, so the rule is not "don't
log secrets", it is **no secret may take that prefix**:

```bash
cd frontend && npm run build
grep -c "sk_test\|sk_live\|whsec_" dist/assets/*.js     # 0
```

### Seeding

`cargo run -p demo -- seed` creates two fixed-id tenants, each with a Stripe
customer, two plans and one subscription, and two users:
**`alice@example.test`** — Owner of Tenant A, Member of Tenant B — and
**`bob@example.test`**, Owner of Tenant B only. Both get the password in
`SEED_USER_PASSWORD` (15 characters or more), hashed with the live Argon2id
parameters; re-seeding resets them to it. The billing half needs four more
variables, read only by the seed: `SEED_PLAN_{A,B}_STRIPE_PRICE_ID` and
`SEED_PLAN_{A,B}_STRIPE_PRODUCT_ID`. Point them at any two prices in your own
test-mode account — the seed mirrors them rather than creating them, so a
wrong id is a display-only mistake. The ids are fixed rather than random,
which is what makes re-running the seed a no-op.

**Both subscriptions seed `incomplete`, not `active` — this is expected.** A
subscription for a customer with no payment method has an open invoice and
nothing to pay it with.

### The demo path

At `http://localhost:5173`:

0. Log in as `alice@example.test`. She has two memberships, so she picks a
   tenant — Tenant A. (Bob, with one, goes straight in.)
1. Subscription shows **incomplete**; invoices and cards are empty. All three
   are correct renderings of real seeded data.
2. **Cancel** it → "no subscription".
3. **Subscribe** → Stripe's hosted Checkout. Pay with `4242 4242 4242 4242`.
4. Return to `/checkout/return` → **pending** until the webhook lands, then
   `active`. Not before.
5. The paid invoice and the attached card appear.
6. Add a second card through the Payment Element — pending, then it appears.
   Set default, detach.
7. Switch tenants in the header; the other tenant's data replaces it. The
   session cookie survives the Checkout round trip and a page refresh.

**Why cancel comes first**, since it looks arbitrary: attaching a card does
not settle the existing subscription's open invoice — that invoice has its own
PaymentIntent, and no route here pays it. Running Checkout alongside an
incomplete subscription would create a *second* subscription at Stripe.
Cancelling first uses only routes that exist and yields, in one hosted flow,
an active subscription, a card, a paid invoice and the webhooks that mirror
them.

If a step hangs, both pending states poll for twenty seconds and then ask
whether `stripe listen` is running — the most likely cause.

### The password reset path

There is no mail provider: the demo's `LogMailer` **writes each mail to the
server log instead of sending it** — the one place a reset link reaches a log,
and `demo` warns about it at every start.

1. Log in as Alice in one tab. In a second (private) window, click **Forgot
   your password?** and enter `alice@example.test`. The confirmation is the
   one any address gets, known or not.
2. Find `DEMO MAILER` in the `cargo run -p demo` output and open its link —
   `http://localhost:5173/reset-password#token=…`. The token is in the
   fragment, and the page strips it from the address bar on load.
3. Set a new password (15–128 characters). No session is started: log in with
   it.
4. Open the same link again: **This link can't be used**.
5. Back in the first tab, the next billing request lands on the login page —
   the reset ended every session Alice had, in both tenants.

Members are API-only in this slice. An Owner can `POST /api/v1/tenant/members`
an address; a new address gets an invitation link in the same log, redeemed on
the same page.

### What it looks like

The demo host is one page: the tenant switcher in the nav, the subscription
panel, then payment methods and invoices side by side. Both screenshots below
are the same build against the same database — only the selected tenant
differs.

**Tenant A**, after running the demo path above: an `active` subscription on
Pro, the card that paid for it plus a second one, and the invoice Stripe
raised. Every value on the page came from the local mirror, not from a live
Stripe call.

![Tenant A: an active Pro subscription, two saved cards, one paid invoice](docs/screenshots/tenant-a.jpg)

**Tenant B**, one select away: no subscription, no cards, no invoices. Nothing
about the request changed except the tenant in the session — the module reads
the tenant from a host-supplied extractor and every repository method is scoped
by it, so this is the isolation the tests assert, shown rather than claimed.

![Tenant B: no subscription, no payment methods, no invoices](docs/screenshots/tenant-b.jpg)

Amounts render from `{amount_minor, currency}` — `49.00 USD` is `4900` and
`"USD"` on the wire, formatted in the browser, because formatting is a
presentation concern and a pre-formatted string would bake a locale into the
API.

### Commands

```bash
cargo test --workspace          # cargo fmt --all / clippy --workspace --all-targets
cargo test -p domain money::    # one test or module by name
cargo doc --workspace --no-deps --open
```

```bash
cd frontend
npm run typecheck   # tsc --noEmit — the lint gate
npm run test        # vitest
npm run build
```

Integration tests bring up a disposable Postgres via `testcontainers` and need
Docker. The unit tests in the library crates do not — `service` and
`identity-service` test every use case, the webhook processor included,
against in-memory doubles.

---

## 4. The API surface

`billing_router` mounts eleven tenant-scoped routes; `webhook_router` serves
`POST /webhooks/stripe` alone. `billing_router` is generic over a host-supplied
tenant extractor — **the tenant never crosses the wire as input**, and ids in
a path are always *local* uuids, resolved to Stripe ids server-side.

**Read** — the local mirror only, no outbound Stripe call:

| Method | Path | Returns |
|---|---|---|
| GET | `/plans` | the tenant's plans |
| GET | `/subscription` | the current subscription, or `null` (200, not 404) |
| GET | `/invoices` | keyset page (`?limit=` 1–100, `?after=` opaque cursor) |
| GET | `/invoices/{id}` | one invoice; unknown id and another tenant's both 404 |
| GET | `/payment-methods` | stored cards (brand / last4 / default only) |

**Write** — each goes through the idempotency ledger; ownership is checked
*before* the outbound call, so a cross-tenant id is a 404 with no call made:

| Method | Path | Does |
|---|---|---|
| POST | `/subscriptions/checkout-session` | Checkout Session; returns the hosted `url` |
| POST | `/subscriptions/{id}/change-plan` | change plan, prorated |
| POST | `/subscriptions/{id}/cancel` | at period end (default) or immediately |
| POST | `/payment-methods/setup-intent` | SetupIntent → `client_secret` |
| POST | `/payment-methods/{id}/default` | set default: Stripe first, then the mirror |
| DELETE | `/payment-methods/{id}` | detach at Stripe, soft-delete the mirror; 204 |

**Webhook**, no tenant context, authenticated by signature:
`POST /webhooks/stripe`.

**Identity** — `identity_router`, merged beside the two above by `demo`:

| Method | Path | Auth | Does |
|---|---|---|---|
| POST | `/auth/login` | — | `200` + `Set-Cookie`; one membership → tenant session, several → pick one |
| POST | `/auth/tenant` | session | scope the session to a tenant; rotates the cookie; not a member → 404 |
| GET, PATCH | `/auth/me` | session | the caller's account and memberships; `PATCH` sets the display name |
| POST | `/auth/password/change` | session | `204`; ends every *other* session |
| POST | `/auth/logout` | session | `204` + clearing cookie |
| POST | `/auth/password-reset/request` | — | `202` and one fixed body, whatever the address |
| POST | `/auth/password-reset/complete` | — | `204`, no cookie; every unusable link is one `400` |
| GET | `/tenant/members` | Owner/Admin | the session tenant's memberships |
| POST | `/tenant/members` | Owner/Admin | `201`, the same body whether or not the address had an account |
| POST | `/tenant/members/{id}/suspend` | Owner/Admin | `204`; another tenant's id is the same `404` as an unknown one |
| POST | `/tenant/members/{id}/deactivate` | Owner/Admin | `204`; `403` if the account also belongs to another tenant — the same `403` as any refusal |
| POST | `/tenant/members/{id}/reactivate` | Owner/Admin | `204`; no sole-tenant rule, since it grants nothing new |
| POST | `/auth/deactivate` | session | `204` + clearing cookie; no tenant need be picked |

The session is a `__Host-session` cookie — `HttpOnly; Secure; SameSite=Strict;
Path=/` — so no script can read it. Every cookie-carrying `POST`, `PATCH`,
`PUT` or `DELETE`, to either module, must come from an origin in
`IDENTITY_ALLOWED_ORIGINS` (`403` otherwise): `SameSite` is the first CSRF
layer, the origin check the second.

Errors are `application/problem+json` (RFC 9457) with a deliberately coarse
`detail` that never carries the raw cause, and a `correlation_id` generated
server-side and logged alongside — never accepted from an inbound header. Both
modules produce the same five members, so the frontend parses them with one
function.

---

## 5. Design decisions

Each is argued at length in the rustdoc on the type that implements it; these
are the decision and the alternative rejected.

**Multi-tenancy — and why not RLS.** `TenantId` is enforced at four
independent layers: the type (a newtype, not a bare `Uuid`), the port
signature (every repository method takes it, so there is no method that could
read across tenants), the SQL (`WHERE tenant_id = $1`), and the tests
(cross-tenant access asserts 404 — the same response as an unknown id, so the
API cannot be used to probe for another tenant's records). The tenant reaches
a handler only from the host's extractor; accepting `tenant_id` as a request
parameter would be textbook IDOR, and the `T: TenantExtractor` bound makes
that a compile-time property. **RLS was considered and deferred:** the module
uses one shared `PgPool`, so RLS would mean `SET LOCAL` on every transaction
and certainty that no query escapes one — trading a greppable predicate for an
implicit session property whose failure mode is a silent full-table read. RLS
becomes right as soon as connections are per-tenant.

**Soft delete.** Rows carry `deleted_at` and are never physically removed —
which is more than a column: unique indexes are partial
(`WHERE deleted_at IS NULL`) so a deleted row does not block a new one with
the same natural key, and `ON DELETE RESTRICT` makes a physical delete fail
loudly rather than cascade quietly.

**Money is integer minor units plus a currency**, everywhere — the domain, the
DTOs and the frontend. Both `Money` fields are private, so an amount cannot
exist without its currency, and `add` is fallible on a currency mismatch.
There is no `f64` in the money path. The wire shape is
`{"amount_minor": 1999, "currency": "EUR"}` — not a float, and not a
preformatted string, which would push locale and rounding into whatever
produced it. The frontend formats with `Math.trunc`/`%`, never `amount / 100`.
**Limitation:** the divisor is 100 — correct for USD, EUR and GBP, wrong for
zero-decimal currencies like JPY.

**Idempotency: the key is persisted before the call, completed after.** A
fingerprint of the request inputs (length-prefixed, so two field splittings
cannot collide) reserves a ledger key for `(tenant, operation, fingerprint)`;
a matching row inside a 23-hour window returns the same key and Stripe
deduplicates the call. **Why `Uuid::new_v4()` at call time is not
idempotency:** a key generated per attempt is a *new* key on the retry, so
Stripe sees an unrelated request and creates a second subscription. Surviving
the failure that caused the retry means being in the database before the call.
The window sits just under Stripe's own 24-hour retention.

**Webhooks invert every assumption the outbound path makes.** The body is raw
`Bytes`, never `Json<T>` — the signature is an HMAC over the exact bytes sent,
and an extractor that deserialised and re-encoded it would break verification
irrecoverably. Verification happens before anything parses. A header can carry
several `v1` signatures during a rotation, so the check accepts if **any**
matches; the timestamp tolerance is five minutes and symmetric, because a
timestamp too far in the future is as suspicious as one too far in the past.
**Dedup is the insert, not a check before it** — every event id hits a unique
constraint and the conflict *is* the signal, leaving no window a redelivery
could slip through. **Ordering is resolved by Postgres:** mirror rows carry the
event's `created_at` and every update is guarded by
`last_event_created_at <= EXCLUDED.last_event_created_at`, so a late event
simply does not apply. The tenant is resolved from the mirror by Stripe
customer id, never read from the payload. Nine event types are handled;
everything else is acknowledged and ignored. **Almost everything returns
200** — Stripe retries non-2xx for days, so a 500 on "we don't handle this"
would queue a permanent retry against a handler that can never succeed.

**Audit trail — append-only, and why soft delete does not apply here.** Every
mirror table carries `deleted_at`; `audit.audit_log` has no such column, on
purpose. A mirror row models *current* state a tenant can retract; an audit
entry models a past fact, which never becomes untrue, so there is nothing to
retract. A soft delete is still an `UPDATE`, and reusing the pattern would
grant the one privilege — write access to an existing row — that lets a
tampered entry look identical to a real one. Enforced at three independent
layers, not by convention: `AuditSink` has exactly one method (no update, no
delete); `audit_log` has no `deleted_at`; and the role the application writes
as, `audit_writer`, is granted `INSERT` and `SELECT` only, nothing more,
checked by the phase's own restricted-role integration test.
**The erasure tension** (a "delete my account" request, in real tension with
"never deletable") is answered by keeping personal data out of the row in the
first place, not by an exception to append-only: every identifying field in
`AuditEntry` is an opaque id — tenant, actor, target — never a name, an
email, or free text (see the type property below). Erasing a person erases
the record that resolves that id to a name, wherever identity is owned; the
audit row survives as a fact that something happened, referencing an id that
may no longer resolve to anything.

**Secrets are structurally unrepresentable, not reviewed for.** `AuditEntry`
has no free-form field anywhere — no `String`, no `HashMap<String, String>`,
no `details: Option<String>` — so a password, token or secret has nowhere in
the type to go. `Actor::User` takes an opaque `SubjectId`, not a raw string;
the crate's own rustdoc includes a `compile_fail` doctest proving
`Actor::User("a string")` does not compile. This is checkable by reading one
file, the same standard the module holds its own `Money` and tenant-isolation
guarantees to.

**One shared database, two independent migrators.** `audit-pg` runs its own
migrations against its own schema (`audit`), tracked in
`audit._sqlx_migrations` rather than the billing module's own
`_sqlx_migrations` — the two migrators run against one Postgres instance
without colliding, in either order, idempotently. A host wires both:
`persistence::run_migrations` and `audit_pg::run_migrations`, as `demo`'s
composition root does. The connecting application role is not `audit_writer`
itself — that role is `NOLOGIN` and carries no password, deliberately kept
out of anything checked into git — a host grants its own role membership at
deploy time: `GRANT audit_writer TO <app role>;`.

**What a host must supply.** `TenantExtractor` (required): an Axum
`FromRequestParts` extractor rejecting with the module's own `ApiError`, which
is what keeps every error in `problem+json`; omitting it is a compile error,
not a runtime 500. `BillingEventSink` (optional): how the module tells a host
something changed. Dedup and the mirror write happen *before* the sink is
called, and a sink failure does not roll back the mirror — the mirror is a
fact about what Stripe said, the sink is a side effect of it. The host
implements it with no Stripe client in its own manifest. **One precondition:**
the webhook route must receive the body unmodified, so a compression or
body-rewriting layer mounted *above* this router breaks verification — and it
looks like a signing-secret mismatch, not like middleware.

**Two router factories, deliberately.** `webhook_router` is not generic: that
route authenticates by signature and has no tenant. On the generic router, a
tenant extractor would become a precondition of it — a regression that
surfaces as Stripe receiving 401s.

**The frontend is a host that installs two features.** `host/` owns the API
client and the session state; `identity/` owns login, tenant picking and
reset; `billing/` owns endpoints, hooks and components. `host/` imports nothing
from `billing/` (`grep -rn "billing/" frontend/src/host/` prints nothing), and
`identity/` and `billing/` never import each other (checked in CI). There is
one `fetch` in the app, and it sends the cookie; no token exists in script.
Every query key carries the tenant id, built through one factory, and the
cache is cleared on switch. State this precisely: **cache partitioning, not
authorization.** The server was never at risk; a key collision would have been
a display bug. **The redirect is a UX signal; the webhook is the truth** —
neither Checkout's return nor the Element's confirmation renders success on
its own; both poll until the mirror reflects it.
---

## 6. The identity module

Users belong to several tenants with one role per membership (`owner`,
`admin`, `member`), authenticate with a password, and hold a server-side
session; a password can be reset by mail; an Owner or Admin can invite and
suspend members. Every account-level change is written to the same audit
journal billing uses. Seven decisions carry it. Each is argued in full in the
rustdoc where the code that implements it lives; these are the short forms.

**1. The boundary held — and what that costs.** Billing's `api` defines
`TenantExtractor` and nothing else about who the caller is. The identity
module's `AuthenticatedSession` extractor is generic over any router state and
reads its service from a request extension, so the host wraps it in a
ten-line newtype, `IdentityTenant`, and billing's routes accept it.
`git diff --stat` over `crates/api`, `domain`, `service`, `persistence` and
`stripe-adapter` across the whole phase prints nothing. Holding it has three
prices: no role-based authorization on billing routes (a Member can cancel the
subscription), no `403` from billing (its one refusal is `401`, which the
frontend reads as logged out), and billing's audit rows still say
`Actor::System`. Each is a deliberate `api` change later, not a host
workaround. → [`crates/demo/src/identity_tenant.rs`](crates/demo/src/identity_tenant.rs)

**2. Argon2id, justified by a memory budget — and not used for tokens.**
`m = 19456 KiB, t = 2, p = 1`, 32-byte output: OWASP's baseline. Every hash
allocates 19 MiB, and hashing runs in `spawn_blocking` behind a 4-permit
semaphore, so peak hashing memory is about 76 MiB however many logins arrive;
excess logins queue instead of allocating. Stored PHC strings carry their
parameters and a successful login rehashes a stale one, so raising `m` after
measuring on real hardware is safe. Session, reset and invitation tokens are
256 random bits stored as SHA-256 and compared in constant time: Argon2 slows
the guessing of *low-entropy* secrets, a 256-bit token cannot be guessed, and
a slow hash on every authenticated request would be a denial-of-service lever.
→ [`argon2_hasher.rs`](crates/identity-service/src/argon2_hasher.rs),
[`token.rs`](crates/identity-domain/src/token.rs)

**3. Timing cannot enumerate accounts, because the request path does no
account-dependent work.** `POST /auth/password-reset/request` parses the
address, counts it, puts it on a bounded queue and answers `202` with one
fixed body. Its handler's state holds a rate limiter and the queue — no
repository — so it *cannot* look the address up. Looking up, issuing the
token, the audit rows and the mail happen afterwards, in a worker. That is
structural, not a simulation of equal work; a test asserts the handler makes
zero repository calls. Login, which must answer, takes the other route: an
unknown address verifies against a dummy Argon2id hash, so both paths pay the
same cost. → [`reset_routes.rs`](crates/identity-api/src/reset_routes.rs)

**4. A reset ends every session, in every tenant, and starts none.** It is
done logged out and often prompted by suspected compromise, so no existing
session deserves the benefit of the doubt; and logging the user in would make
a mail link equivalent to a password. An *authenticated* password change keeps
the caller's own session and ends the others, because that caller just proved
the current password. The reset token is single-use (consumed under a row lock,
so two concurrent completions succeed at most once), superseded by a newer
one (`UNIQUE (user_id, purpose)`), and lives 15 minutes; an invitation lives
72 hours and can only ever set a *first* password.
→ [`reset.rs`](crates/identity-service/src/reset.rs)

**5. Rate limits by address *and* by IP, because each stops what the other
cannot.** A per-address limit stops a targeted attack on one victim spread
across many IPs; a per-IP limit stops one source working through many
addresses. Login: 10 per (address, IP) and 100 per IP per 15 minutes — never
per address alone, which would let anyone lock a victim out. Reset request: 3
per address per hour, **silently** (still `202`, or the limit itself would
reveal the address is being targeted), and 20 per IP (`429`). Reset
completion: 20 per IP per 15 minutes. The address key is a SHA-256
fingerprint, IPv6 is keyed by its /64, and `X-Forwarded-For` is believed only
from a configured proxy. Counters are in memory — per process, lost on
restart — behind a port a shared store can implement.
→ [`limits.rs`](crates/identity-api/src/limits.rs)

**6. Account-level audit events fan out: one row per active membership.**
`audit_log` requires a tenant, and a password reset happens to a person, not
a tenant. Each tenant the person belongs to gets the row, in the same
transaction and with one correlation id, and the memberships are read under
`FOR SHARE` so the list is the one true at commit. The rejected alternative —
a nullable tenant scope — meant a migration on `audit_log`, a changed
`AuditEntry::new`, every billing call site touched, and tenant views that
follow current membership rather than membership at the time. Two costs,
named: one fact is N rows (count distinct correlation ids), and a user with no
active membership gets no row. → [`fan_out.rs`](crates/identity-pg/src/fan_out.rs)

**7. Deactivation is global, so a tenant may not always do it.** Roles are
tenant-scoped; closing an account is not. An Owner of one tenant ending an
account that also belongs to another would reach across the boundary the
module exists to hold, and could lock a second tenant out of its own Owner.
So an admin may deactivate **only** an account whose one active membership is
their tenant; anything else is refused, and the tenant-scoped instrument
stays suspension. The constraint is decided in the repository, inside the
write's own transaction, because a use case that read the memberships first
would be deciding on a set that may already have changed. **Both refusals —
"you may not" and "they belong to another tenant" — are one `403` with one
body**, because a distinct answer would disclose a membership of a tenant the
caller has no part in; tests pin that at the service and at the wire.
Reactivation is admin-only and carries no such rule: it grants no access the
account did not already have, and a deactivated user has no session to ask
from. → [`deactivation.rs`](crates/identity-service/src/deactivation.rs)

**Named limits of this slice.** Members are added active, without the
invitee's consent; there is no role change and no last-Owner guard (two
Owners can suspend each other). A membership inserted in another tenant
*concurrently* with a deactivation is not prevented — row locks bind existing
rows, not future ones — so an account can end up deactivated in a tenant that
had just added it, which that tenant can undo by reactivating. An
account-level event for a user with **no** active membership is written to no
journal at all, since `audit_log` requires a tenant. The reset queue and the rate-limit
counters are in-process: a restart loses queued requests (the user asks
again), and several instances would need shared counters and an outbox.

### Where each password-reset property is proved

Reset is the part of this module most worth checking line by line, so each
required property is pinned by a named test rather than left to the prose
above. The labels `R1`–`R11` appear in those tests' own doc comments.

| Property | Test | Where |
|---|---|---|
| R1 — token hashed at rest; a leaked table cannot reset an account | `stored_reset_token_contains_no_verifier_bytes` | [`identity-pg/tests/reset_issue.rs`](crates/identity-pg/tests/reset_issue.rs) |
| R2 — single use, invalidated on consumption | `a_reset_token_cannot_be_used_twice` | [`identity-pg/tests/reset_complete.rs`](crates/identity-pg/tests/reset_complete.rs) |
| R3 — invalidated on issuing a new one | `issuing_a_reset_token_invalidates_the_previous_one` | [`identity-pg/tests/reset_issue.rs`](crates/identity-pg/tests/reset_issue.rs) |
| R4 — two concurrent completions succeed at most once | `two_concurrent_completions_succeed_at_most_once` | [`identity-pg/tests/reset_complete.rs`](crates/identity-pg/tests/reset_complete.rs) |
| R5 — short, explicit expiry (15 minutes) | `a_reset_token_is_refused_after_fifteen_minutes` | [`identity-service/src/reset.rs`](crates/identity-service/src/reset.rs) |
| R6 — constant-time comparison | `verification_uses_constant_time_equality` | [`identity-domain/src/token.rs`](crates/identity-domain/src/token.rs) |
| R7 — no enumeration: the answer is byte-identical | `reset_request_body_is_identical_for_known_and_unknown_addresses` | [`identity-api/tests/reset_request.rs`](crates/identity-api/tests/reset_request.rs) |
| R8 — no enumeration: response *time* cannot differ either | `reset_request_handler_cannot_reach_the_user_repository` | [`identity-api/tests/reset_request.rs`](crates/identity-api/tests/reset_request.rs) |
| R9 — existing sessions invalidated, in every tenant | `completing_a_reset_deletes_sessions_in_every_tenant` | [`identity-pg/tests/reset_complete.rs`](crates/identity-pg/tests/reset_complete.rs) |
| R10 — and no new session is issued | `completing_a_reset_sets_no_cookie` | [`identity-api/tests/reset_complete.rs`](crates/identity-api/tests/reset_complete.rs) |
| R11 — rate limited per address, silently | `address_limit_is_silent_and_applies_to_unknown_addresses` | [`identity-api/tests/reset_request.rs`](crates/identity-api/tests/reset_request.rs) |
| R11 — and per IP, with `429` | `ip_limit_returns_429` | [`identity-api/tests/reset_request.rs`](crates/identity-api/tests/reset_request.rs) |

Two of these are worth reading rather than trusting: R1 serialises the whole
stored row to JSON and asserts the verifier appears in no column in any
encoding, and R8 asserts the handler made **zero** use-case calls — a
structural claim, not a timing measurement, which would be flaky or
meaningless as a unit test.
