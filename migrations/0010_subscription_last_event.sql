-- Ordering anchor for webhook processing (init-spec.md §10.2): each
-- subscription row stores the `created` timestamp of the last event applied
-- to it, so an older, out-of-order delivery can be detected and skipped
-- rather than blindly overwriting newer state. Nullable with no default --
-- NULL means "no event has been applied to this row yet", which is true of
-- every row created before this migration.
ALTER TABLE billing.subscriptions
    ADD COLUMN IF NOT EXISTS last_event_created_at TIMESTAMPTZ;

-- Backs the webhook path's tenant resolution (§10.3): stripe_customer_id ->
-- local customer row -> tenant_id. Plain, not unique -- see the phase-4a plan
-- (Open Question 2) for why a partial-unique variant was considered and
-- deferred.
CREATE INDEX IF NOT EXISTS customers_stripe_customer_id_idx
    ON billing.customers (stripe_customer_id);
