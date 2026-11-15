-- What the REST API (step 2.07) needs from the database. Documented in
-- docs/db-schema.md.
--
-- 1. An account records the client that opened it and the idempotency key
--    it used, so a retried "open account" returns the same account. The
--    client is also the account's owner, for step 2.08's ownership checks.
-- 2. Adjustment requests: an adjustment is requested by one person and
--    approved by another (INV-25) in two calls, so the request waits here.
--
-- New SQLSTATE:
--   MZ009  forbidden adjustment request change

ALTER TABLE accounts
    ADD COLUMN opened_by UUID,
    ADD COLUMN open_key  TEXT CHECK (open_key ~ '^[A-Za-z0-9._:-]{1,128}$'),
    ADD CONSTRAINT accounts_open_key_unique UNIQUE (opened_by, open_key),
    ADD CONSTRAINT accounts_open_key_with_client CHECK ((open_key IS NULL) OR (opened_by IS NOT NULL));

-- Who opened an account never changes.
CREATE FUNCTION mezgeb_accounts_opener_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.opened_by IS DISTINCT FROM OLD.opened_by OR NEW.open_key IS DISTINCT FROM OLD.open_key THEN
        RAISE EXCEPTION 'account %: who opened it cannot change', OLD.id
            USING ERRCODE = 'MZ005';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER accounts_opener_guard
    BEFORE UPDATE ON accounts
    FOR EACH ROW EXECUTE FUNCTION mezgeb_accounts_opener_guard();

CREATE TABLE adjustment_requests (
    id              UUID        PRIMARY KEY,
    client_id       UUID        NOT NULL,
    idempotency_key TEXT        NOT NULL CHECK (idempotency_key ~ '^[A-Za-z0-9._:-]{1,128}$'),
    -- pending -> approving -> approved, or pending -> rejected; approving ->
    -- pending when the ledger refuses the lines.
    status          TEXT        NOT NULL CHECK (status IN ('pending', 'approving', 'approved', 'rejected')),
    reason_code     TEXT        NOT NULL,
    -- The requested lines: [{"account": uuid, "side": "debit"|"credit",
    -- "amount": minor units, "currency": "ETB"}, ...].
    lines           JSONB       NOT NULL CHECK (jsonb_typeof(lines) = 'array'),
    metadata        JSONB       NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(metadata) = 'object'),
    note            TEXT        CHECK (length(note) <= 500),
    requested_by    TEXT        NOT NULL CHECK (requested_by ~ '^[A-Za-z0-9._:@-]{1,128}$'),
    requested_at    TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    decided_by      TEXT        CHECK (decided_by ~ '^[A-Za-z0-9._:@-]{1,128}$'),
    decided_at      TIMESTAMPTZ,
    decision_note   TEXT        CHECK (length(decision_note) <= 500),
    entry_id        UUID        REFERENCES journal_entries (id),

    CONSTRAINT adjustment_requests_key_unique UNIQUE (client_id, idempotency_key),
    CONSTRAINT adjustment_requests_reason_fk
        FOREIGN KEY (reason_code) REFERENCES reason_codes (code),
    -- INV-25: the checker is never the maker.
    CONSTRAINT adjustment_requests_two_people CHECK (decided_by IS DISTINCT FROM requested_by),
    CONSTRAINT adjustment_requests_decided CHECK (
        CASE status
            WHEN 'pending' THEN decided_by IS NULL AND entry_id IS NULL
            WHEN 'approving' THEN decided_by IS NOT NULL AND entry_id IS NULL
            WHEN 'approved' THEN decided_by IS NOT NULL AND decided_at IS NOT NULL AND entry_id IS NOT NULL
            WHEN 'rejected' THEN decided_by IS NOT NULL AND decided_at IS NOT NULL AND entry_id IS NULL
        END
    )
);

-- Only a request's decision columns change, and only along its states.
CREATE FUNCTION mezgeb_adjustment_requests_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id <> OLD.id OR NEW.client_id <> OLD.client_id
       OR NEW.idempotency_key <> OLD.idempotency_key OR NEW.reason_code <> OLD.reason_code
       OR NEW.lines <> OLD.lines OR NEW.metadata <> OLD.metadata
       OR NEW.note IS DISTINCT FROM OLD.note OR NEW.requested_by <> OLD.requested_by
       OR NEW.requested_at <> OLD.requested_at THEN
        RAISE EXCEPTION 'adjustment request %: only its decision can change', OLD.id
            USING ERRCODE = 'MZ009';
    END IF;
    IF NOT ((OLD.status = 'pending' AND NEW.status IN ('approving', 'rejected'))
            OR (OLD.status = 'approving' AND NEW.status = 'approved'
                AND NEW.decided_by = OLD.decided_by)
            -- The ledger refused the lines: back to pending, undecided.
            OR (OLD.status = 'approving' AND NEW.status = 'pending'
                AND NEW.decided_by IS NULL)) THEN
        RAISE EXCEPTION 'adjustment request %: % -> % is not allowed', OLD.id, OLD.status, NEW.status
            USING ERRCODE = 'MZ009';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER adjustment_requests_guard
    BEFORE UPDATE ON adjustment_requests
    FOR EACH ROW EXECUTE FUNCTION mezgeb_adjustment_requests_guard();

CREATE TRIGGER adjustment_requests_no_delete
    BEFORE DELETE ON adjustment_requests
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

GRANT SELECT, INSERT ON adjustment_requests TO mezgeb_app;
GRANT UPDATE (status, decided_by, decided_at, decision_note, entry_id) ON adjustment_requests TO mezgeb_app;
GRANT SELECT ON adjustment_requests TO mezgeb_readonly, mezgeb_verifier;
