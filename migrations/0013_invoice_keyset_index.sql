-- Supports GET /invoices' keyset pagination (docs/spec/phase-4b-read-routes.md
-- decision 4). The query is tenant-scoped and orders by
-- (created_at DESC, id DESC), seeking with a row-value comparison
-- `(created_at, id) < ($ts, $id)`. Index column order follows that: tenant_id
-- first, because every query filters on it, then the two sort keys in the
-- order they are compared, each DESC to match the ORDER BY so Postgres reads
-- the index forward.
--
-- Partial on `deleted_at IS NULL`: the query carries that same predicate, so
-- a partial index is both smaller and usable without a recheck.
--
-- IF NOT EXISTS, like every migration in this tree, so the double-run
-- migration test (S3) stays true.
CREATE INDEX IF NOT EXISTS invoices_tenant_created_id_idx
    ON billing.invoices (tenant_id, created_at DESC, id DESC)
    WHERE deleted_at IS NULL;
