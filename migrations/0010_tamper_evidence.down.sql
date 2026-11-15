-- Reverts 0010_tamper_evidence.up.sql.

DROP TABLE checkpoints;
DROP TABLE audit_events;
DROP TABLE entry_chain;
DROP TABLE chain_heads;
DROP TABLE signing_keys;
