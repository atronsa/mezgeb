-- Reverts 0012_offline_receipts.up.sql.

DROP TABLE offline_settings;
DROP TABLE offline_receipt_conflicts;
DROP TABLE offline_receipts;
DROP FUNCTION mezgeb_offline_receipt_guard();
