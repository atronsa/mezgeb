-- Reverts 0003_corrections.up.sql.

ALTER TABLE journal_entries
    DROP CONSTRAINT journal_entries_authorization,
    DROP CONSTRAINT journal_entries_reason_fk,
    DROP COLUMN approved_by,
    DROP COLUMN requested_by,
    DROP COLUMN reason_code,
    DROP COLUMN kind;

DROP TABLE reason_codes;
