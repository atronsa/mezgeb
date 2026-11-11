-- Database roles (docs/db-schema.md#roles). Roles belong to the whole
-- PostgreSQL cluster, so they are created once, by a superuser, before any
-- migration runs. Docker runs this file on the container's first start; CI
-- runs it with psql.
--
-- The passwords are for local development and CI only. Production sets its
-- own, from a secret store (step 3.12).

-- Owns the schema and runs migrations. Never used by the running service.
CREATE ROLE mezgeb_owner LOGIN CREATEDB PASSWORD 'mezgeb_owner';

-- The running service. May insert and read journal rows and update account
-- totals and status, nothing else (granted per table by the migrations).
CREATE ROLE mezgeb_app LOGIN PASSWORD 'mezgeb_app';

-- Reports, reconciliation and support tools. Read only.
CREATE ROLE mezgeb_readonly LOGIN PASSWORD 'mezgeb_readonly';

CREATE DATABASE mezgeb OWNER mezgeb_owner;
