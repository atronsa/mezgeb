-- Reversals and adjustments with reasons and maker-checker (step 1.12).
-- Documented in docs/db-schema.md.
--
-- Every entry has a kind. A reversal (INV-14) and an adjustment carry a
-- reason code and the person who asked for them; an adjustment also carries
-- a second, different person who approved it (INV-25). The engine checks
-- this first; the database checks it again.

CREATE TABLE reason_codes (
    code        TEXT PRIMARY KEY CHECK (code ~ '^[a-z0-9_]{1,64}$'),
    entry_kind  TEXT NOT NULL CHECK (entry_kind IN ('reversal', 'adjustment')),
    description TEXT NOT NULL CHECK (length(description) BETWEEN 1 AND 200),

    -- Lets an entry reference a code together with its own kind.
    CONSTRAINT reason_codes_code_kind_unique UNIQUE (code, entry_kind)
);

-- The same list as REASON_CODES in crates/core/src/authorization.rs.
INSERT INTO reason_codes (code, entry_kind, description) VALUES
    ('booking_error', 'reversal', 'Posted by mistake: wrong account, amount or customer'),
    ('duplicate_posting', 'reversal', 'The same event was posted twice'),
    ('goodwill_credit', 'adjustment', 'Goodwill credit to a customer, paid from the platform''s own money'),
    ('compensation', 'adjustment', 'Compensation owed to a customer or merchant'),
    ('bank_charge', 'adjustment', 'A bank charge booked after the fact'),
    ('write_off', 'adjustment', 'An amount that will not be recovered'),
    ('data_migration', 'adjustment', 'A balance carried over from another system');

ALTER TABLE journal_entries
    ADD COLUMN kind TEXT NOT NULL DEFAULT 'standard'
        CHECK (kind IN ('standard', 'reversal', 'adjustment')),
    ADD COLUMN reason_code TEXT,
    ADD COLUMN requested_by TEXT CHECK (requested_by ~ '^[A-Za-z0-9._:@-]{1,128}$'),
    ADD COLUMN approved_by TEXT CHECK (approved_by ~ '^[A-Za-z0-9._:@-]{1,128}$');

-- A reason must exist and be meant for this kind of entry: a reversal
-- cannot use an adjustment reason, and the other way round.
ALTER TABLE journal_entries
    ADD CONSTRAINT journal_entries_reason_fk
        FOREIGN KEY (reason_code, kind) REFERENCES reason_codes (code, entry_kind);

-- INV-14 and INV-25: who and why, per kind. NOT VALID: it applies to every
-- new entry; entries posted before this migration are history and are never
-- changed (INV-9).
ALTER TABLE journal_entries
    ADD CONSTRAINT journal_entries_authorization CHECK (
        CASE kind
            WHEN 'standard' THEN
                reverses_entry_id IS NULL
                AND reason_code IS NULL AND requested_by IS NULL AND approved_by IS NULL
            WHEN 'reversal' THEN
                reverses_entry_id IS NOT NULL
                AND reason_code IS NOT NULL AND requested_by IS NOT NULL
                AND approved_by IS DISTINCT FROM requested_by
            WHEN 'adjustment' THEN
                reverses_entry_id IS NULL
                AND reason_code IS NOT NULL AND requested_by IS NOT NULL
                AND approved_by IS NOT NULL AND approved_by <> requested_by
        END
    ) NOT VALID;

GRANT SELECT ON reason_codes TO mezgeb_app, mezgeb_readonly;
