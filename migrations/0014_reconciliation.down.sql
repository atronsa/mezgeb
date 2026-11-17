-- Reverts 0014_reconciliation.up.sql.

DROP TABLE recon_breaks;
DROP TABLE recon_items;
DROP TABLE recon_runs;
DROP FUNCTION mezgeb_recon_break_guard();
DROP FUNCTION mezgeb_recon_item_guard();
ALTER TABLE vouchers DROP COLUMN reported_last_counter;
