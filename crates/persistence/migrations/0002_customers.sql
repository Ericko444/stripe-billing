CREATE TABLE IF NOT EXISTS billing.customers (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    stripe_customer_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS customers_tenant_id_idx
    ON billing.customers (tenant_id);

CREATE UNIQUE INDEX IF NOT EXISTS customers_tenant_id_stripe_customer_id_key
    ON billing.customers (tenant_id, stripe_customer_id)
    WHERE deleted_at IS NULL;
