-- Draft schema (spec §3, §8). Amounts are BIGINT minor units.
-- IDs are 128-bit, stored as UUID.

CREATE TABLE accounts (
    id               UUID PRIMARY KEY,
    ledger           INTEGER  NOT NULL CHECK (ledger > 0),
    code             SMALLINT NOT NULL,
    flags            SMALLINT NOT NULL DEFAULT 0,
    debits_pending   BIGINT   NOT NULL DEFAULT 0 CHECK (debits_pending  >= 0),
    debits_posted    BIGINT   NOT NULL DEFAULT 0 CHECK (debits_posted   >= 0),
    credits_pending  BIGINT   NOT NULL DEFAULT 0 CHECK (credits_pending >= 0),
    credits_posted   BIGINT   NOT NULL DEFAULT 0 CHECK (credits_posted  >= 0),
    user_data        UUID,
    created_at       BIGINT   NOT NULL
);

CREATE TABLE transfers (
    id                 UUID PRIMARY KEY,
    debit_account_id   UUID     NOT NULL REFERENCES accounts(id),
    credit_account_id  UUID     NOT NULL REFERENCES accounts(id),
    amount             BIGINT   NOT NULL CHECK (amount > 0),
    ledger             INTEGER  NOT NULL,
    code               SMALLINT NOT NULL,
    flags              SMALLINT NOT NULL DEFAULT 0,
    pending_id         UUID REFERENCES transfers(id),
    timeout            INTEGER  NOT NULL DEFAULT 0,
    user_data          UUID,
    event_time         BIGINT,
    recorded_time      BIGINT   NOT NULL UNIQUE,
    prev_hash          BYTEA    NOT NULL,
    hash               BYTEA    NOT NULL,
    CHECK (debit_account_id <> credit_account_id)
);

CREATE INDEX transfers_debit_idx  ON transfers (debit_account_id, recorded_time);
CREATE INDEX transfers_credit_idx ON transfers (credit_account_id, recorded_time);

-- Offline sync (spec §5).
CREATE TABLE vouchers (
    voucher_id               UUID PRIMARY KEY,
    device_id                UUID     NOT NULL,
    tier                     SMALLINT NOT NULL,
    account_id               UUID     NOT NULL REFERENCES accounts(id),
    reserved_account_id      UUID     NOT NULL REFERENCES accounts(id),
    reservation_transfer_id  UUID     NOT NULL REFERENCES transfers(id),
    amount                   BIGINT   NOT NULL CHECK (amount > 0),
    counter_start            BIGINT   NOT NULL,
    expires_at               BIGINT   NOT NULL,
    device_public_key        BYTEA    NOT NULL,
    issuer_signature         BYTEA    NOT NULL
);

CREATE TABLE receipts (
    device_id      UUID     NOT NULL,
    counter        BIGINT   NOT NULL,
    voucher_id     UUID     NOT NULL REFERENCES vouchers(voucher_id),
    receipt_hash   BYTEA    NOT NULL,
    outcome        SMALLINT NOT NULL,
    transfer_id    UUID REFERENCES transfers(id),
    received_at    BIGINT   NOT NULL,
    PRIMARY KEY (device_id, counter)   -- INV-10
);

-- TODO (spec §7): REVOKE UPDATE, DELETE on transfers/receipts from the app role.
-- TODO (spec §8): partition transfers by month.
