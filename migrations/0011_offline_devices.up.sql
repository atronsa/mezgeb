-- Offline devices and vouchers (step 2.11, ADR-0019, docs/offline/protocol.md
-- §4–§7). Documented in docs/db-schema.md#offline-devices-and-vouchers.
--
-- A device is a payer's card or phone with a P-256 key in its secure
-- hardware. A voucher is an allowance reserved in its own escrow
-- sub-account (2050:vch-…) by entry E16 and signed by the issuer key.
-- Receipts and settlement follow in step 2.12.

-- The public half of each P-256 issuer key that has signed device
-- certificates or vouchers. Retired keys stay, so old signatures remain
-- verifiable. The secret key is never in the database.
CREATE TABLE offline_issuer_keys (
    key_id     TEXT        PRIMARY KEY CHECK (key_id ~ '^[0-9a-f]{16}$'),
    public_key BYTEA       NOT NULL UNIQUE CHECK (length(public_key) = 33),
    added_at   TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

-- A client's own limits for a tier, at or below the protocol's maxima
-- (protocol §6). Without a row, tiers A and B have the maxima and tier C is
-- disabled: a client must enable software keys on purpose.
CREATE TABLE offline_tier_limits (
    client_id         UUID        NOT NULL REFERENCES clients (id),
    tier              TEXT        NOT NULL CHECK (tier IN ('A', 'B', 'C')),
    enabled           BOOLEAN     NOT NULL,
    max_voucher_minor BIGINT      NOT NULL CHECK (max_voucher_minor > 0),
    lifetime_micros   BIGINT      NOT NULL CHECK (lifetime_micros > 0),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),

    PRIMARY KEY (client_id, tier),
    -- Lowered, never raised: 2,000 / 1,000 / 200 ETB and 7 / 3 / 1 days.
    CONSTRAINT offline_tier_limits_within_maxima CHECK (
        max_voucher_minor <= CASE tier WHEN 'A' THEN 200000 WHEN 'B' THEN 100000 ELSE 20000 END
        AND lifetime_micros <= CASE tier WHEN 'A' THEN 7 WHEN 'B' THEN 3 ELSE 1 END * 86400000000
    )
);

CREATE TABLE devices (
    id                    UUID        PRIMARY KEY,
    client_id             UUID        NOT NULL REFERENCES clients (id),
    -- The owner's wallet (2010:…), which funds the device's vouchers.
    owner_account_id      UUID        NOT NULL REFERENCES accounts (id),
    public_key            BYTEA       NOT NULL UNIQUE CHECK (length(public_key) = 33),
    key_id                TEXT        NOT NULL UNIQUE CHECK (key_id ~ '^[0-9a-f]{16}$'),
    tier                  TEXT        NOT NULL CHECK (tier IN ('A', 'B', 'C')),
    -- How the tier was decided: the attestation's kind and what was checked.
    attestation           TEXT        NOT NULL CHECK (attestation IN ('none', 'card', 'android')),
    attestation_detail    TEXT        NOT NULL DEFAULT '' CHECK (length(attestation_detail) <= 1024),
    label                 TEXT        NOT NULL CHECK (length(label) BETWEEN 1 AND 100),
    -- The device certificate (protocol §5).
    issued_at             TIMESTAMPTZ NOT NULL,
    valid_until           TIMESTAMPTZ NOT NULL CHECK (valid_until > issued_at),
    issuer_key_id         TEXT        NOT NULL REFERENCES offline_issuer_keys (key_id),
    certificate_signature BYTEA       NOT NULL CHECK (length(certificate_signature) = 64),
    -- The first counter its next voucher authorises.
    next_counter          BIGINT      NOT NULL DEFAULT 1 CHECK (next_counter >= 1),
    status                TEXT        NOT NULL DEFAULT 'active'
                          CHECK (status IN ('active', 'frozen', 'lost', 'revoked')),
    status_reason         TEXT        CHECK (length(status_reason) <= 200),
    status_changed_at     TIMESTAMPTZ
);

CREATE INDEX devices_owner ON devices (owner_account_id);
CREATE INDEX devices_client ON devices (client_id);

CREATE TABLE vouchers (
    id                UUID        PRIMARY KEY,
    device_id         UUID        NOT NULL REFERENCES devices (id),
    client_id         UUID        NOT NULL REFERENCES clients (id),
    -- The wallet the escrow came from, and the escrow itself.
    owner_account_id  UUID        NOT NULL REFERENCES accounts (id),
    escrow_account_id UUID        NOT NULL UNIQUE REFERENCES accounts (id),
    currency          TEXT        NOT NULL REFERENCES currencies (code),
    amount_minor      BIGINT      NOT NULL CHECK (amount_minor > 0),
    counter_start     BIGINT      NOT NULL CHECK (counter_start >= 1),
    issued_at         TIMESTAMPTZ NOT NULL,
    expires_at        TIMESTAMPTZ NOT NULL CHECK (expires_at > issued_at),
    issuer_key_id     TEXT        NOT NULL REFERENCES offline_issuer_keys (key_id),
    signature         BYTEA       NOT NULL CHECK (length(signature) = 64),
    -- E16: the entry that reserved the escrow.
    entry_id          UUID        NOT NULL UNIQUE REFERENCES journal_entries (id),
    status            TEXT        NOT NULL DEFAULT 'active'
                      CHECK (status IN ('active', 'grace', 'frozen', 'returned', 'closed', 'ended')),
    status_changed_at TIMESTAMPTZ
);

-- One open voucher per device (ADR-0019 decision 3).
CREATE UNIQUE INDEX vouchers_one_open_per_device
    ON vouchers (device_id) WHERE status IN ('active', 'grace', 'frozen');
CREATE INDEX vouchers_owner ON vouchers (owner_account_id) WHERE status IN ('active', 'grace', 'frozen');

-- Signed records never change; only a state moves.
CREATE TRIGGER offline_issuer_keys_no_change
    BEFORE UPDATE OR DELETE ON offline_issuer_keys
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

GRANT SELECT, INSERT ON offline_issuer_keys, devices, vouchers TO mezgeb_app;
GRANT UPDATE (status, status_reason, status_changed_at, next_counter) ON devices TO mezgeb_app;
GRANT UPDATE (status, status_changed_at) ON vouchers TO mezgeb_app;
GRANT SELECT ON offline_tier_limits TO mezgeb_app;
GRANT SELECT ON offline_issuer_keys, offline_tier_limits, devices, vouchers
    TO mezgeb_readonly, mezgeb_verifier;
-- `mezgeb-cli offline` sets a client's tier limits and revokes devices.
GRANT SELECT, INSERT, UPDATE ON offline_tier_limits TO mezgeb_admin;
GRANT SELECT ON devices, vouchers TO mezgeb_admin;
GRANT UPDATE (status, status_reason, status_changed_at) ON devices TO mezgeb_admin;
