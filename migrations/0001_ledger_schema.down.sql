-- Reverts 0001_ledger_schema.up.sql. Dropping a table also drops its
-- triggers and grants. Roles are cluster-wide and are left in place.

DROP TABLE journal_lines;
DROP TABLE journal_entries;
DROP TABLE accounts;
DROP TABLE currencies;

DROP FUNCTION mezgeb_reject_change();
DROP FUNCTION mezgeb_check_reversal_mirror();
DROP FUNCTION mezgeb_check_reversal_target();
DROP FUNCTION mezgeb_check_entry_balanced();
DROP FUNCTION mezgeb_check_line_account_open();
DROP FUNCTION mezgeb_accounts_guard();
