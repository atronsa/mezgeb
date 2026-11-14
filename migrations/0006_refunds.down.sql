-- Reverts 0006_refunds.up.sql.

DROP TRIGGER journal_entries_refund_rules ON journal_entries;
DROP FUNCTION mezgeb_check_refund();
DROP TABLE refund_tallies;
DROP FUNCTION mezgeb_refund_tallies_guard();

ALTER TABLE journal_entries
    DROP CONSTRAINT journal_entries_authorization,
    DROP CONSTRAINT journal_entries_kind_check,
    ADD CONSTRAINT journal_entries_kind_check
        CHECK (kind IN ('standard', 'reversal', 'adjustment'));

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

DROP INDEX journal_entries_refunds_idx;
ALTER TABLE journal_entries DROP COLUMN refunds_entry_id;
