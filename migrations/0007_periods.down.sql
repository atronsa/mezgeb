-- Reverts 0007_periods.up.sql.

DROP TABLE period_snapshots;
DROP TRIGGER journal_lines_period_open ON journal_lines;
DROP TRIGGER journal_entries_period_open_at_commit ON journal_entries;
DROP TRIGGER journal_entries_period_open ON journal_entries;
DROP FUNCTION mezgeb_check_period_open();
DROP INDEX journal_entries_period_idx;
ALTER TABLE journal_entries
    DROP CONSTRAINT journal_entries_has_period,
    DROP COLUMN period_id;
DROP TABLE periods;
DROP FUNCTION mezgeb_periods_guard();
