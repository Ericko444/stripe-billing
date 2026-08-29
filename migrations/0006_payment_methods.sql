CREATE TABLE IF NOT EXISTS billing.payment_methods (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    customer_id UUID NOT NULL REFERENCES billing.customers (id) ON DELETE RESTRICT,
    stripe_payment_method_id TEXT NOT NULL,
    brand TEXT NOT NULL,
    last4 TEXT NOT NULL,
    is_default BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS payment_methods_tenant_id_idx
    ON billing.payment_methods (tenant_id);

CREATE UNIQUE INDEX IF NOT EXISTS payment_methods_tenant_id_stripe_payment_method_id_key
    ON billing.payment_methods (tenant_id, stripe_payment_method_id)
    WHERE deleted_at IS NULL;
