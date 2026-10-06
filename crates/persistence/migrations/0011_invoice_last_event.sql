-- Ordering anchor for invoice webhook processing (init-spec.md §10.2),
-- mirroring 0010 for subscriptions: each invoice row stores the `created`
-- timestamp of the last event applied to it, so an older, out-of-order
-- delivery can be detected and skipped rather than overwriting newer state.
-- Nullable with no default -- NULL means "no event has been applied to this
-- row yet", true of every row created before this migration and of the
-- first mirror write for a brand-new invoice.
ALTER TABLE billing.invoices
    ADD COLUMN IF NOT EXISTS last_event_created_at TIMESTAMPTZ;

-- Backs the webhook path's invoice resolution: stripe_invoice_id -> local
-- invoice row. Plain, not unique -- the tenant-scoped partial-unique index
-- from 0005 already guards uniqueness; this one only needs to make the
-- lookup an index scan.
CREATE INDEX IF NOT EXISTS invoices_stripe_invoice_id_idx
    ON billing.invoices (stripe_invoice_id);
