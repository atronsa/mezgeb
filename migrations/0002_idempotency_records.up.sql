-- Idempotency records (step 1.11, ADR-0008). Documented in docs/db-schema.md.
--
-- One row per successfully posted request: which client, which key, the
-- SHA-256 fingerprint of the request, and the entry it created. It is
-- written in the same transaction as the entry, so a request either
-- happened once, with its record, or not at all.
--
-- The key is claimed first (INSERT … ON CONFLICT DO NOTHING), before any
-- account is locked: a concurrent request with the same key waits on this
-- table's primary key until the first one commits or rolls back.

-- Lets a record point at its entry together with the entry's own client and
-- key, so the two can never disagree.
ALTER TABLE journal_entries
    ADD CONSTRAINT journal_entries_id_client_key_unique UNIQUE (id, client_id, idempotency_key);

CREATE TABLE idempotency_records (
    client_id       UUID        NOT NULL,
    idempotency_key TEXT        NOT NULL CHECK (idempotency_key ~ '^[A-Za-z0-9._:-]{1,128}$'),
    request_hash    BYTEA       NOT NULL CHECK (octet_length(request_hash) = 32),
    entry_id        UUID        NOT NULL UNIQUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- INV-13: a key is used once per client.
    PRIMARY KEY (client_id, idempotency_key),
    -- The record and its entry share client and key. Deferred, because the
    -- key is claimed before the entry is inserted.
    CONSTRAINT idempotency_records_entry_fk
        FOREIGN KEY (entry_id, client_id, idempotency_key)
        REFERENCES journal_entries (id, client_id, idempotency_key)
        DEFERRABLE INITIALLY DEFERRED
);

-- A record never changes. Old records may be deleted by a retention job
-- (docs/db-schema.md#idempotency-and-retention); the service's role cannot.
CREATE TRIGGER idempotency_records_append_only
    BEFORE UPDATE ON idempotency_records
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

GRANT SELECT, INSERT ON idempotency_records TO mezgeb_app;
GRANT SELECT ON idempotency_records TO mezgeb_readonly;
