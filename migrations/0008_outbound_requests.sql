CREATE TABLE IF NOT EXISTS billing.outbound_requests (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    operation TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    stripe_object_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS outbound_requests_idempotency_key_key
    ON billing.outbound_requests (idempotency_key);
