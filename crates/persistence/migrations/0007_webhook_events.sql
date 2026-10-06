CREATE TABLE IF NOT EXISTS billing.webhook_events (
    id UUID PRIMARY KEY,
    tenant_id UUID,
    stripe_event_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS webhook_events_stripe_event_id_key
    ON billing.webhook_events (stripe_event_id);
