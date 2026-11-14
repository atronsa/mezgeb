-- Accounting periods, closing and snapshots (step 2.05, INV-10).
-- Documented in docs/db-schema.md and docs/runbooks/close-period.md.
--
-- Every entry belongs to the period that was open when it was posted. A
-- period is open (postings go to it), closing (no more postings; being
-- verified and snapshotted) or closed (its balances never change). Exactly
-- one period is open at a time; closing one opens the next.
--
-- A period holds the entries with after_sequence < sequence <= last_sequence:
-- postings take a shared advisory lock, and the switch to the next period
-- takes it exclusively, so every entry of a period is committed before the
-- period stops taking postings.
--
-- New SQLSTATE:
--   MZ008  posting into a period that is not open, or a forbidden period change

CREATE TABLE periods (
    id             BIGINT      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    label          TEXT        NOT NULL UNIQUE CHECK (label ~ '^[A-Za-z0-9._:-]{1,64}$'),
    status         TEXT        NOT NULL CHECK (status IN ('open', 'closing', 'closed')),
    opened_at      TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    -- The period holds entries with after_sequence < sequence <= last_sequence.
    after_sequence BIGINT      NOT NULL CHECK (after_sequence >= 0),
    last_sequence  BIGINT      CHECK (last_sequence >= after_sequence),
    closing_at     TIMESTAMPTZ,
    closed_at      TIMESTAMPTZ,

    CONSTRAINT periods_open_has_no_end
        CHECK ((status = 'open') = (last_sequence IS NULL AND closing_at IS NULL)),
    CONSTRAINT periods_closed_has_close_time
        CHECK ((status = 'closed') = (closed_at IS NOT NULL))
);

-- INV-10: one open period, and at most one being closed.
CREATE UNIQUE INDEX periods_one_open ON periods ((true)) WHERE status = 'open';
CREATE UNIQUE INDEX periods_one_closing ON periods ((true)) WHERE status = 'closing';

-- A period only moves forward: open -> closing (recording where it ends),
-- then closing -> closed. Nothing else about it ever changes.
CREATE FUNCTION mezgeb_periods_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id <> OLD.id OR NEW.label <> OLD.label OR NEW.opened_at <> OLD.opened_at
       OR NEW.after_sequence <> OLD.after_sequence THEN
        RAISE EXCEPTION 'period %: only its status and end can change', OLD.id
            USING ERRCODE = 'MZ008';
    END IF;
    IF NOT ((OLD.status = 'open' AND NEW.status = 'closing')
            OR (OLD.status = 'closing' AND NEW.status = 'closed'
                AND NEW.last_sequence = OLD.last_sequence
                AND NEW.closing_at = OLD.closing_at)) THEN
        RAISE EXCEPTION 'period %: % -> % is not allowed', OLD.id, OLD.status, NEW.status
            USING ERRCODE = 'MZ008';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER periods_guard
    BEFORE UPDATE ON periods
    FOR EACH ROW EXECUTE FUNCTION mezgeb_periods_guard();

CREATE TRIGGER periods_no_delete
    BEFORE DELETE ON periods
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

-- The first period starts after whatever was posted before periods existed.
INSERT INTO periods (label, status, after_sequence)
SELECT 'initial', 'open', coalesce(max(sequence), 0) FROM journal_entries;

-- Each entry's period. NOT VALID: entries posted before this migration are
-- history and are never changed (INV-9); every new entry has a period.
ALTER TABLE journal_entries
    ADD COLUMN period_id BIGINT REFERENCES periods (id),
    ADD CONSTRAINT journal_entries_has_period CHECK (period_id IS NOT NULL) NOT VALID;

CREATE INDEX journal_entries_period_idx ON journal_entries (period_id);

-- INV-10: an entry, and each of its lines, goes only into an open period.
-- Checked when the row is written, and again for the entry at commit: a
-- transaction that wrote into a period which stopped taking postings before
-- it committed is refused.
CREATE FUNCTION mezgeb_check_period_open() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    checked_id    UUID;
    period_status TEXT;
BEGIN
    IF TG_TABLE_NAME = 'journal_entries' THEN
        checked_id := NEW.id;
        SELECT status INTO period_status FROM periods WHERE id = NEW.period_id;
    ELSE
        checked_id := NEW.entry_id;
        SELECT p.status INTO period_status
          FROM journal_entries e JOIN periods p ON p.id = e.period_id
         WHERE e.id = NEW.entry_id;
        -- Entries from before periods existed have no period.
        IF NOT FOUND THEN
            RETURN NULL;
        END IF;
    END IF;
    IF period_status IS DISTINCT FROM 'open' THEN
        RAISE EXCEPTION 'entry % is in a period that is %, not open',
            checked_id, coalesce(period_status, 'missing')
            USING ERRCODE = 'MZ008';
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER journal_entries_period_open
    AFTER INSERT ON journal_entries
    FOR EACH ROW EXECUTE FUNCTION mezgeb_check_period_open();

CREATE CONSTRAINT TRIGGER journal_entries_period_open_at_commit
    AFTER INSERT ON journal_entries
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION mezgeb_check_period_open();

CREATE TRIGGER journal_lines_period_open
    AFTER INSERT ON journal_lines
    FOR EACH ROW EXECUTE FUNCTION mezgeb_check_period_open();

-- Each account's totals at the end of a closed period: the sum of every line
-- of every entry up to the period's last_sequence.
CREATE TABLE period_snapshots (
    period_id     BIGINT NOT NULL REFERENCES periods (id),
    account_id    UUID   NOT NULL REFERENCES accounts (id),
    debits_total  BIGINT NOT NULL CHECK (debits_total >= 0),
    credits_total BIGINT NOT NULL CHECK (credits_total >= 0),
    PRIMARY KEY (period_id, account_id)
);

CREATE TRIGGER period_snapshots_append_only
    BEFORE UPDATE OR DELETE ON period_snapshots
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

GRANT SELECT, INSERT ON periods, period_snapshots TO mezgeb_app;
GRANT UPDATE (status, last_sequence, closing_at, closed_at) ON periods TO mezgeb_app;
GRANT SELECT ON periods, period_snapshots TO mezgeb_readonly, mezgeb_verifier;
