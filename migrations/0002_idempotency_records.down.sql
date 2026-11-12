-- Reverts 0002_idempotency_records.up.sql.

DROP TABLE idempotency_records;

ALTER TABLE journal_entries DROP CONSTRAINT journal_entries_id_client_key_unique;
