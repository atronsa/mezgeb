-- The client administration role (step 2.08).
--
-- A new database gets this automatically. On a database created before
-- step 2.08, run it once:
--   docker compose exec -T postgres psql -U postgres -f - < db/init/03-admin.sql

-- Runs `mezgeb-cli client`: creates clients and credentials and revokes
-- them, nothing else (docs/db-schema.md#roles). The service itself can read
-- credentials but never create one, so a compromised service cannot mint
-- itself a credential.
CREATE ROLE mezgeb_admin LOGIN PASSWORD 'mezgeb_admin';
