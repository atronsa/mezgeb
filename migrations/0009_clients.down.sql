-- Reverts 0009_clients.up.sql.

DROP TABLE access_tokens;
DROP TABLE client_credentials;
DROP TABLE clients;
DROP FUNCTION mezgeb_client_credentials_guard();
DROP FUNCTION mezgeb_clients_guard();
