-- Reverts 0013_offline_expiry_and_disputes.up.sql.

DROP TABLE offline_dispute_payments;
DROP TABLE offline_disputes;
DROP TABLE offline_shortfalls;
ALTER TABLE offline_receipts DROP CONSTRAINT offline_receipts_outcome_check;
ALTER TABLE offline_receipts ADD CONSTRAINT offline_receipts_outcome_check CHECK (outcome IN (
    'settled', 'settled_late', 'waiting_for_gap', 'conflict', 'frozen', 'over_limit', 'expired'));
ALTER TABLE offline_settings ALTER COLUMN fee_bps DROP DEFAULT;
ALTER TABLE offline_settings DROP COLUMN late_deadline_days;
ALTER TABLE vouchers
    DROP CONSTRAINT vouchers_ended_when,
    DROP CONSTRAINT vouchers_returned_when,
    DROP COLUMN ended_at,
    DROP COLUMN return_entry_id,
    DROP COLUMN returned_at;
