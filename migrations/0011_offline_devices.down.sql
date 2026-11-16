-- Reverts 0011_offline_devices.up.sql.

DROP TABLE vouchers;
DROP TABLE devices;
DROP TABLE offline_tier_limits;
DROP TABLE offline_issuer_keys;
