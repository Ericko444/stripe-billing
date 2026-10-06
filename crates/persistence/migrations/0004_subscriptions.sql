CREATE TABLE IF NOT EXISTS billing.subscriptions (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    customer_id UUID NOT NULL REFERENCES billing.customers (id) ON DELETE RESTRICT,
    plan_id UUID NOT NULL REFERENCES billing.plans (id) ON DELETE RESTRICT,
    stripe_subscription_id TEXT NOT NULL,
    stripe_subscription_item_id TEXT NOT NULL,
    status TEXT NOT NULL,
    current_period_start TIMESTAMPTZ NOT NULL,
    current_period_end TIMESTAMPTZ NOT NULL,
    cancel_at_period_end BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS subscriptions_tenant_id_idx
    ON billing.subscriptions (tenant_id);

CREATE UNIQUE INDEX IF NOT EXISTS subscriptions_tenant_id_stripe_subscription_id_key
    ON billing.subscriptions (tenant_id, stripe_subscription_id)
    WHERE deleted_at IS NULL;
