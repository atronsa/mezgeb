-- Reverts 0008_api_support.up.sql.

DROP TABLE adjustment_requests;
DROP FUNCTION mezgeb_adjustment_requests_guard();
DROP TRIGGER accounts_opener_guard ON accounts;
DROP FUNCTION mezgeb_accounts_opener_guard();
ALTER TABLE accounts
    DROP CONSTRAINT accounts_open_key_with_client,
    DROP CONSTRAINT accounts_open_key_unique,
    DROP COLUMN open_key,
    DROP COLUMN opened_by;
