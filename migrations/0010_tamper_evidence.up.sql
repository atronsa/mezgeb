-- Tamper evidence (step 2.09, ADR-0018): the hash chain over the journal
-- with a signed receipt per entry, the audit events and their chain, the
-- signed checkpoints, and the public signing keys. Documented in
-- docs/db-schema.md#tamper-evidence.
--
-- The chains are append-only like the journal: rows are never changed or
-- deleted. Only the two heads move, each one row that every append locks.
-- Entries posted before this migration are not chained; verification
-- reports them as history before the chain.

-- The public half of each key that has signed receipts or checkpoints.
-- Retired keys stay, so old signatures remain verifiable.
CREATE TABLE signing_keys (
    key_id     TEXT        PRIMARY KEY CHECK (key_id ~ '^[0-9a-f]{16}$'),
    public_key BYTEA       NOT NULL UNIQUE CHECK (length(public_key) = 32),
    added_at   TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

-- The head of each chain: its last position and hash. One row each.
CREATE TABLE chain_heads (
    chain    TEXT   PRIMARY KEY CHECK (chain IN ('entries', 'audit')),
    position BIGINT NOT NULL CHECK (position >= 0),
    hash     BYTEA  NOT NULL CHECK (length(hash) = 32)
);
INSERT INTO chain_heads (chain, position, hash)
VALUES ('entries', 0, '\x0000000000000000000000000000000000000000000000000000000000000000'),
       ('audit', 0, '\x0000000000000000000000000000000000000000000000000000000000000000');

-- One link per entry, written in the entry's own posting transaction.
-- The signature over the link is the entry's ledger receipt (INV-26); it is
-- absent only where the engine runs without a signing key (tests).
CREATE TABLE entry_chain (
    position   BIGINT      PRIMARY KEY CHECK (position > 0),
    entry_id   UUID        NOT NULL UNIQUE REFERENCES journal_entries (id),
    prev_hash  BYTEA       NOT NULL CHECK (length(prev_hash) = 32),
    hash       BYTEA       NOT NULL CHECK (length(hash) = 32),
    key_id     TEXT        REFERENCES signing_keys (key_id),
    signature  BYTEA       CHECK (length(signature) = 64),
    chained_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),

    CONSTRAINT entry_chain_signed CHECK ((key_id IS NULL) = (signature IS NULL))
);

-- Who did what, when (TM-11, TM-16): every API mutation and refusal, and
-- every read across clients. `details` is stored as the exact text the
-- hash covers.
CREATE TABLE audit_events (
    position   BIGINT      PRIMARY KEY CHECK (position > 0),
    at         TIMESTAMPTZ NOT NULL,
    client_id  UUID,
    actor      TEXT        CHECK (length(actor) <= 128),
    -- An API request as `METHOD /route/{pattern}`, or a named action such
    -- as `token.issue` or `credential.revoke`.
    action     TEXT        NOT NULL CHECK (
        action ~ '^[A-Z]{3,7} /[A-Za-z0-9_./{}-]{0,120}$' OR action ~ '^[a-z_.]{1,64}$'
    ),
    target     TEXT        CHECK (length(target) <= 256),
    request_id TEXT        CHECK (length(request_id) <= 64),
    outcome    TEXT        NOT NULL CHECK (outcome ~ '^[a-z_]{1,32}$'),
    details    TEXT        NOT NULL DEFAULT '{}' CHECK (length(details) <= 8192),
    prev_hash  BYTEA       NOT NULL CHECK (length(prev_hash) = 32),
    hash       BYTEA       NOT NULL CHECK (length(hash) = 32)
);

CREATE INDEX audit_events_client ON audit_events (client_id, position);

-- The heads of both chains at a period's close, signed.
CREATE TABLE checkpoints (
    -- Numbered 1, 2, ... under the chain heads' locks.
    id             BIGINT      PRIMARY KEY CHECK (id > 0),
    period_id      BIGINT      NOT NULL UNIQUE REFERENCES periods (id),
    position       BIGINT      NOT NULL CHECK (position >= 0),
    hash           BYTEA       NOT NULL CHECK (length(hash) = 32),
    audit_position BIGINT      NOT NULL CHECK (audit_position >= 0),
    audit_hash     BYTEA       NOT NULL CHECK (length(audit_hash) = 32),
    at             TIMESTAMPTZ NOT NULL,
    key_id         TEXT        NOT NULL REFERENCES signing_keys (key_id),
    signature      BYTEA       NOT NULL CHECK (length(signature) = 64)
);

-- Append-only, like the journal (INV-9).
CREATE TRIGGER entry_chain_no_change
    BEFORE UPDATE OR DELETE ON entry_chain
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();
CREATE TRIGGER audit_events_no_change
    BEFORE UPDATE OR DELETE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();
CREATE TRIGGER checkpoints_no_change
    BEFORE UPDATE OR DELETE ON checkpoints
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();
CREATE TRIGGER signing_keys_no_change
    BEFORE UPDATE OR DELETE ON signing_keys
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

GRANT SELECT, INSERT ON signing_keys, entry_chain, audit_events, checkpoints TO mezgeb_app;
GRANT SELECT ON chain_heads TO mezgeb_app;
GRANT UPDATE (position, hash) ON chain_heads TO mezgeb_app;
GRANT SELECT ON signing_keys, chain_heads, entry_chain, audit_events, checkpoints
    TO mezgeb_readonly, mezgeb_verifier;
-- `mezgeb-cli client` records its changes to clients and credentials.
GRANT SELECT, INSERT ON audit_events TO mezgeb_admin;
GRANT SELECT ON chain_heads TO mezgeb_admin;
GRANT UPDATE (position, hash) ON chain_heads TO mezgeb_admin;
