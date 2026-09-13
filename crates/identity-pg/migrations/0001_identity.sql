-- The identity module's schema. Its own schema and its own migrator table
-- (`identity._sqlx_migrations`, see `src/lib.rs`), so it shares a database
-- with the billing module and the audit journal without coordinating with
-- either.
CREATE SCHEMA IF NOT EXISTS identity;

-- The tenant registry. The billing module has no tenants table, only
-- `tenant_id` columns; this is where a tenant id is issued, and the uuid is
-- the only thing the two modules share.
CREATE TABLE IF NOT EXISTS identity.tenants (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A person, global across tenants. One address, one account: the address is
-- stored normalised (trimmed, lowercased) and unique.
--
-- `password_hash` is an Argon2id PHC string, or NULL for an invited user who
-- has not yet set one -- such a user cannot log in, because there is nothing
-- to verify against.
--
-- `deactivated_at` is a state the session lookup honours, not a soft delete:
-- the row keeps resolving the opaque ids in `audit.audit_log`.
CREATE TABLE IF NOT EXISTS identity.users (
    id UUID PRIMARY KEY,
    email_normalized TEXT NOT NULL UNIQUE,
    password_hash TEXT,
    display_name TEXT NOT NULL DEFAULT '',
    deactivated_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A user's place in one tenant, with one role. The unique pair is what makes
-- "one role per membership" true, and it is the target of the sessions
-- foreign key below.
CREATE TABLE IF NOT EXISTS identity.memberships (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES identity.users (id),
    tenant_id UUID NOT NULL REFERENCES identity.tenants (id),
    role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    status TEXT NOT NULL CHECK (status IN ('active', 'suspended')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, tenant_id)
);

CREATE INDEX IF NOT EXISTS memberships_tenant_id_idx
    ON identity.memberships (tenant_id);

-- One row per live session. The token is a SplitToken: `selector` finds the
-- row, and only SHA-256 of the verifier is kept, so a copy of this table
-- cannot be replayed as a cookie. Lengths are checked here as well as in the
-- type, so a bug that stored the wrong thing fails loudly.
--
-- `tenant_id` is NULL for the short-lived session that exists only to pick a
-- tenant. When set, the composite foreign key means a tenant-scoped session
-- cannot exist without the membership it is scoped to (MATCH SIMPLE: a NULL
-- tenant skips the check).
--
-- Revocation is DELETE. A revoked session has no history worth keeping here;
-- the fact that it was revoked is written to `audit.audit_log`.
CREATE TABLE IF NOT EXISTS identity.sessions (
    id UUID PRIMARY KEY,
    selector BYTEA NOT NULL UNIQUE CHECK (octet_length(selector) = 16),
    verifier_hash BYTEA NOT NULL CHECK (octet_length(verifier_hash) = 32),
    user_id UUID NOT NULL REFERENCES identity.users (id),
    tenant_id UUID REFERENCES identity.tenants (id),
    authenticated_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    CHECK (expires_at > authenticated_at),
    FOREIGN KEY (user_id, tenant_id) REFERENCES identity.memberships (user_id, tenant_id)
);

CREATE INDEX IF NOT EXISTS sessions_user_id_idx
    ON identity.sessions (user_id);

-- Reset and invitation links, same SplitToken shape as sessions.
--
-- UNIQUE (user_id, purpose) is the "issuing a new token invalidates the
-- previous one" rule, enforced by Postgres rather than by the order of two
-- statements: a second outstanding token of the same purpose cannot be
-- inserted until the first is deleted. Consumption and supersession are both
-- DELETE, for the same reason as sessions.
CREATE TABLE IF NOT EXISTS identity.password_tokens (
    id UUID PRIMARY KEY,
    selector BYTEA NOT NULL UNIQUE CHECK (octet_length(selector) = 16),
    verifier_hash BYTEA NOT NULL CHECK (octet_length(verifier_hash) = 32),
    user_id UUID NOT NULL REFERENCES identity.users (id),
    purpose TEXT NOT NULL CHECK (purpose IN ('password_reset', 'invitation')),
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    CHECK (expires_at > created_at),
    UNIQUE (user_id, purpose)
);
