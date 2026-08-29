CREATE TABLE IF NOT EXISTS billing.plans (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    stripe_price_id TEXT,
    stripe_product_id TEXT,
    name TEXT,
    amount_minor BIGINT,
    currency CHAR(3),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS plans_tenant_id_idx
    ON billing.plans (tenant_id);

CREATE UNIQUE INDEX IF NOT EXISTS plans_tenant_id_stripe_price_id_key
    ON billing.plans (tenant_id, stripe_price_id)
    WHERE deleted_at IS NULL;
