-- The verifier role and a time limit on posting transactions (step 1.13).
--
-- A new database gets this automatically. On a database created before
-- step 1.13, run it once:
--   docker compose exec -T postgres psql -U postgres -f - < db/init/02-verifier.sql

-- Runs `mezgeb-cli verify`: reads the ledger and records verification runs,
-- nothing else (docs/db-schema.md#roles).
CREATE ROLE mezgeb_verifier LOGIN PASSWORD 'mezgeb_verifier';

-- No posting transaction may stay open longer than 30 seconds. Incremental
-- verification relies on it: an entry recorded more than 60 seconds ago is
-- either committed or will never appear (docs/db-schema.md#verification).
ALTER ROLE mezgeb_app SET transaction_timeout = '30s';
