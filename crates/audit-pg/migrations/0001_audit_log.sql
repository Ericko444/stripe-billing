-- Its own schema, tracked by its own migrator: `audit-pg` runs against the
-- same Postgres instance as the billing module's `migrations/`, and the two
-- migration sets must not collide. `audit_pg::run_migrations` (see
-- `src/lib.rs`) points sqlx's migration table at `audit._sqlx_migrations`
-- instead of the default `_sqlx_migrations`, which is what makes running
-- both migrators against one database safe.
CREATE SCHEMA IF NOT EXISTS audit;

-- No `deleted_at`, no mutable status column -- the soft-delete pattern the
-- rest of this module uses does not apply here. See the crate's rustdoc for
-- why: a mirror row models current state a tenant may retract; this table
-- models a past fact, which never becomes untrue.
CREATE TABLE IF NOT EXISTS audit.audit_log (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    actor_kind TEXT NOT NULL,
    actor_subject_id UUID,
    action TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id UUID,
    occurred_at TIMESTAMPTZ NOT NULL,
    correlation_id UUID NOT NULL
);

CREATE INDEX IF NOT EXISTS audit_log_tenant_id_idx
    ON audit.audit_log (tenant_id);

CREATE INDEX IF NOT EXISTS audit_log_correlation_id_idx
    ON audit.audit_log (correlation_id);

-- Append-only's second layer -- the first is `AuditSink` having no update or
-- delete method; the third is this phase's atomicity tests. `audit_writer`
-- is a group role, never a login: whichever role the application connects
-- as is granted membership in it and inherits exactly INSERT and SELECT.
-- There is no UPDATE, DELETE or TRUNCATE grant here, and none is added
-- later -- a role that cannot rewrite history cannot be talked into it by a
-- bug two crates away. Left `NOLOGIN` and password-less deliberately: a
-- credential does not belong in a migration file checked into git: a host
-- grants its own connecting role membership in `audit_writer` at deploy
-- time (`GRANT audit_writer TO <app role>;`).
DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'audit_writer') THEN
        CREATE ROLE audit_writer NOLOGIN;
    END IF;
END
$$;

REVOKE ALL ON audit.audit_log FROM PUBLIC;
GRANT USAGE ON SCHEMA audit TO audit_writer;
GRANT INSERT, SELECT ON audit.audit_log TO audit_writer;
