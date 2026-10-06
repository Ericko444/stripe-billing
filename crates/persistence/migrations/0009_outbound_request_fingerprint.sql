-- Guards the idempotency ledger's reserve race: two concurrent requests for
-- the same (tenant, operation, fingerprint) must not both find "no row" and
-- each mint their own key. Partial on `completed_at IS NULL` so only one
-- *in-flight* reservation per fingerprint is possible -- once a row
-- completes, the same fingerprint recurring later (a genuinely new
-- operation, after the idempotency key has expired at Stripe) is free to
-- insert a second row.
CREATE UNIQUE INDEX IF NOT EXISTS outbound_requests_fingerprint_inflight_key
    ON billing.outbound_requests (tenant_id, operation, request_fingerprint)
    WHERE completed_at IS NULL;

-- Non-unique: backs find_by_fingerprint's "most recent matching row" lookup.
CREATE INDEX IF NOT EXISTS outbound_requests_fingerprint_lookup_idx
    ON billing.outbound_requests (tenant_id, operation, request_fingerprint, created_at DESC);
