-- Ordering anchor for payment-method webhook processing (init-spec.md
-- §10.2), the same treatment 0010 gave subscriptions and 0011 invoices: the
-- last applied event's `created` timestamp, so a stale `attached` cannot
-- resurrect a card a newer `detached` removed. Nullable, no default -- NULL
-- means no event has been applied to the row yet.
ALTER TABLE billing.payment_methods
    ADD COLUMN IF NOT EXISTS last_event_created_at TIMESTAMPTZ;

-- Backs the webhook path's lookup: stripe_payment_method_id -> local row.
-- Plain, not unique -- the tenant-scoped partial-unique index from 0006
-- already guards uniqueness.
CREATE INDEX IF NOT EXISTS payment_methods_stripe_payment_method_id_idx
    ON billing.payment_methods (stripe_payment_method_id);
