-- Clients, their credentials and their access tokens (step 2.08,
-- ADR-0017). Documented in docs/db-schema.md#clients-and-credentials.
--
-- A client is a platform: it owns the accounts it opens, its idempotency
-- keys and its rate limit. A credential is one key for one actor of a
-- client, with roles from the permission matrix. Its ID is the OAuth
-- client_id; its secret is stored only as an Argon2id hash. An access token
-- is stored only as its SHA-256 and is valid only while its credential and
-- its client are active.
--
-- Who may do what:
--   mezgeb_admin  creates clients and credentials, disables and revokes them
--   mezgeb_app    reads them (to check a secret or a token), issues and
--                 deletes access tokens; never creates a credential
--
-- New SQLSTATE:
--   MZ010  forbidden client or credential change

CREATE TABLE clients (
    id              UUID        PRIMARY KEY,
    name            TEXT        NOT NULL UNIQUE CHECK (name ~ '^[a-z0-9][a-z0-9._-]{0,63}$'),
    status          TEXT        NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    -- The rate limit: a token bucket of `burst` requests, refilled at
    -- `rate_per_second`.
    rate_per_second INTEGER     NOT NULL DEFAULT 100 CHECK (rate_per_second BETWEEN 1 AND 100000),
    burst           INTEGER     NOT NULL DEFAULT 200 CHECK (burst BETWEEN 1 AND 100000),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    disabled_at     TIMESTAMPTZ,

    CONSTRAINT clients_disabled CHECK ((status = 'disabled') = (disabled_at IS NOT NULL))
);

CREATE TABLE client_credentials (
    -- The OAuth client_id.
    id          UUID        PRIMARY KEY,
    client_id   UUID        NOT NULL REFERENCES clients (id),
    -- The person or service acting: recorded on corrections, and the
    -- identity maker-checker compares (INV-25).
    actor       TEXT        NOT NULL CHECK (actor ~ '^[A-Za-z0-9._:@-]{1,128}$'),
    -- The roles of crates/api/permissions.toml.
    roles       TEXT[]      NOT NULL CHECK (
        cardinality(roles) BETWEEN 1 AND 6
        AND roles <@ ARRAY['poster', 'reader', 'operator', 'approver', 'auditor', 'admin']
        -- Separation of duties: one credential never both requests and
        -- approves corrections.
        AND NOT (roles @> ARRAY['operator', 'approver'])
    ),
    -- An Argon2id PHC string.
    secret_hash TEXT        NOT NULL CHECK (secret_hash LIKE '$argon2id$%'),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    revoked_at  TIMESTAMPTZ
);

CREATE INDEX client_credentials_client ON client_credentials (client_id);

CREATE TABLE access_tokens (
    -- SHA-256 of the token.
    token_hash    BYTEA       PRIMARY KEY CHECK (length(token_hash) = 32),
    credential_id UUID        NOT NULL REFERENCES client_credentials (id),
    -- The scopes granted, at most what the credential's roles allow.
    scopes        TEXT[]      NOT NULL,
    issued_at     TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at    TIMESTAMPTZ NOT NULL,

    CONSTRAINT access_tokens_lifetime CHECK (expires_at > issued_at)
);

CREATE INDEX access_tokens_expiry ON access_tokens (expires_at);
CREATE INDEX access_tokens_credential ON access_tokens (credential_id);

-- A client changes only its status and rate limit; once disabled, it stays
-- disabled.
CREATE FUNCTION mezgeb_clients_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id <> OLD.id OR NEW.name <> OLD.name OR NEW.created_at <> OLD.created_at
       OR (OLD.status = 'disabled' AND NEW.status <> 'disabled')
       OR (OLD.disabled_at IS NOT NULL AND NEW.disabled_at IS DISTINCT FROM OLD.disabled_at) THEN
        RAISE EXCEPTION 'client %: forbidden change', OLD.id
            USING ERRCODE = 'MZ010';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER clients_guard
    BEFORE UPDATE ON clients
    FOR EACH ROW EXECUTE FUNCTION mezgeb_clients_guard();

-- A credential changes only once: when it is revoked.
CREATE FUNCTION mezgeb_client_credentials_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id <> OLD.id OR NEW.client_id <> OLD.client_id OR NEW.actor <> OLD.actor
       OR NEW.roles <> OLD.roles OR NEW.secret_hash <> OLD.secret_hash
       OR NEW.created_at <> OLD.created_at
       OR OLD.revoked_at IS NOT NULL OR NEW.revoked_at IS NULL THEN
        RAISE EXCEPTION 'credential %: only revoking it is allowed', OLD.id
            USING ERRCODE = 'MZ010';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER client_credentials_guard
    BEFORE UPDATE ON client_credentials
    FOR EACH ROW EXECUTE FUNCTION mezgeb_client_credentials_guard();

-- Clients and credentials are history: never deleted.
CREATE TRIGGER clients_no_delete
    BEFORE DELETE ON clients
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();
CREATE TRIGGER client_credentials_no_delete
    BEFORE DELETE ON client_credentials
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

GRANT SELECT, INSERT ON clients, client_credentials TO mezgeb_admin;
GRANT UPDATE (status, disabled_at, rate_per_second, burst) ON clients TO mezgeb_admin;
GRANT UPDATE (revoked_at) ON client_credentials TO mezgeb_admin;
GRANT SELECT, DELETE ON access_tokens TO mezgeb_admin;

GRANT SELECT ON clients, client_credentials TO mezgeb_app;
GRANT SELECT, INSERT, DELETE ON access_tokens TO mezgeb_app;

GRANT SELECT ON clients TO mezgeb_readonly, mezgeb_verifier;
GRANT SELECT (id, client_id, actor, roles, created_at, revoked_at)
    ON client_credentials TO mezgeb_readonly, mezgeb_verifier;
