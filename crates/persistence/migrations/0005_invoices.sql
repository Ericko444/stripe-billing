CREATE TABLE IF NOT EXISTS billing.invoices (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    customer_id UUID NOT NULL REFERENCES billing.customers (id) ON DELETE RESTRICT,
    subscription_id UUID REFERENCES billing.subscriptions (id) ON DELETE RESTRICT,
    stripe_invoice_id TEXT NOT NULL,
    amount_minor BIGINT NOT NULL,
    currency CHAR(3) NOT NULL,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS invoices_tenant_id_idx
    ON billing.invoices (tenant_id);

CREATE UNIQUE INDEX IF NOT EXISTS invoices_tenant_id_stripe_invoice_id_key
    ON billing.invoices (tenant_id, stripe_invoice_id)
    WHERE deleted_at IS NULL;
