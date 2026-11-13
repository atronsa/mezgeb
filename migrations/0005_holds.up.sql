-- Holds (step 1.14, ADR-0004). Documented in docs/db-schema.md.
--
-- A hold reserves money by a real posting into its own held account, then
-- ends exactly once: captured, released or expired. The engine checks every
-- rule first; the database checks them again.

CREATE TABLE holds (
    id              UUID        PRIMARY KEY,
    account_id      UUID        NOT NULL REFERENCES accounts (id),
    -- INV-18: each hold has its own held account, used by no other hold.
    held_account_id UUID        NOT NULL UNIQUE REFERENCES accounts (id),
    amount          BIGINT      NOT NULL CHECK (amount > 0),
    currency        TEXT        NOT NULL REFERENCES currencies (code),
    -- INV-17: a capture never exceeds the hold.
    captured        BIGINT      NOT NULL DEFAULT 0 CHECK (captured >= 0 AND captured <= amount),
    status          TEXT        NOT NULL DEFAULT 'active'
                    CHECK (status IN ('active', 'captured', 'released', 'expired')),
    expires_at      TIMESTAMPTZ NOT NULL,
    place_entry_id  UUID        NOT NULL UNIQUE REFERENCES journal_entries (id),
    close_entry_id  UUID        UNIQUE REFERENCES journal_entries (id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT holds_distinct_accounts CHECK (held_account_id <> account_id),
    -- INV-16: an ended hold names the entry that ended it, an active one none.
    CONSTRAINT holds_closed_by_entry CHECK ((status = 'active') = (close_entry_id IS NULL)),
    -- Only a captured hold has a captured amount.
    CONSTRAINT holds_captured_only_when_captured CHECK ((captured > 0) = (status = 'captured'))
);

-- The expiry worker's queue: active holds by expiry time.
CREATE INDEX holds_due_idx ON holds (expires_at) WHERE status = 'active';

-- INV-16: a hold ends at most once, and only its status, captured amount and
-- closing entry can change, from active, once.
CREATE FUNCTION mezgeb_holds_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.status <> 'active' THEN
        RAISE EXCEPTION 'hold % has already ended (%)', OLD.id, OLD.status
            USING ERRCODE = 'MZ006';
    END IF;
    IF NEW.status = 'active'
       OR NEW.id <> OLD.id
       OR NEW.account_id <> OLD.account_id
       OR NEW.held_account_id <> OLD.held_account_id
       OR NEW.amount <> OLD.amount
       OR NEW.currency <> OLD.currency
       OR NEW.expires_at <> OLD.expires_at
       OR NEW.place_entry_id <> OLD.place_entry_id
       OR NEW.created_at <> OLD.created_at THEN
        RAISE EXCEPTION 'hold %: only ending it (status, captured, closing entry) is allowed', OLD.id
            USING ERRCODE = 'MZ006';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER holds_guard
    BEFORE UPDATE ON holds
    FOR EACH ROW EXECUTE FUNCTION mezgeb_holds_guard();

CREATE TRIGGER holds_no_delete
    BEFORE DELETE ON holds
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

GRANT SELECT, INSERT ON holds TO mezgeb_app;
GRANT UPDATE (status, captured, close_entry_id) ON holds TO mezgeb_app;
GRANT SELECT ON holds TO mezgeb_readonly, mezgeb_verifier;
