-- Offline receipts (step 2.12, ADR-0019, docs/offline/protocol.md §9–§11).
-- Documented in docs/db-schema.md#offline-receipts.
--
-- Each receipt is stored the first time it arrives, whatever its outcome,
-- by (device_id, counter): that record is what makes a second upload a
-- DUPLICATE (same digest) or a CONFLICT (another digest), whoever uploads
-- it (INV-20, INV-21). A conflicting second receipt is kept as evidence in
-- offline_receipt_conflicts. Settlement posts E17 and records its entry.

CREATE TABLE offline_receipts (
    device_id         UUID        NOT NULL REFERENCES devices (id),
    counter           BIGINT      NOT NULL CHECK (counter >= 1),
    voucher_id        UUID        NOT NULL REFERENCES vouchers (id),
    -- SHA-256 of the body; the signature is not part of it.
    digest            BYTEA       NOT NULL CHECK (length(digest) = 32),
    prev_digest       BYTEA       NOT NULL CHECK (length(prev_digest) = 32),
    -- The exact bytes the device signed, and its signature.
    body              BYTEA       NOT NULL CHECK (length(body) <= 1024),
    signature         BYTEA       NOT NULL CHECK (length(signature) = 64),
    payee_account_id  UUID        NOT NULL REFERENCES accounts (id),
    currency          TEXT        NOT NULL REFERENCES currencies (code),
    amount_minor      BIGINT      NOT NULL CHECK (amount_minor > 0),
    -- The device's clock: recorded, never trusted.
    event_time        TIMESTAMPTZ NOT NULL,
    outcome           TEXT        NOT NULL CHECK (outcome IN (
                          'settled', 'settled_late', 'waiting_for_gap', 'conflict',
                          'frozen', 'over_limit', 'expired')),
    -- The entry that paid it, and from where (protocol §16).
    entry_id          UUID        UNIQUE REFERENCES journal_entries (id),
    from_escrow_minor BIGINT      NOT NULL DEFAULT 0 CHECK (from_escrow_minor >= 0),
    from_wallet_minor BIGINT      NOT NULL DEFAULT 0 CHECK (from_wallet_minor >= 0),
    from_loss_minor   BIGINT      NOT NULL DEFAULT 0 CHECK (from_loss_minor >= 0),
    -- Settled after its gap timed out, without checking its link (§11).
    gap_skipped       BOOLEAN     NOT NULL DEFAULT false,
    -- The client whose upload first recorded it.
    uploaded_by       UUID        NOT NULL REFERENCES clients (id),
    received_at       TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    settled_at        TIMESTAMPTZ,

    PRIMARY KEY (device_id, counter),
    -- Paid exactly when settled, and in full.
    CONSTRAINT offline_receipts_paid_when_settled CHECK (
        (outcome IN ('settled', 'settled_late'))
            = (entry_id IS NOT NULL AND settled_at IS NOT NULL)
    ),
    CONSTRAINT offline_receipts_paid_in_full CHECK (
        outcome NOT IN ('settled', 'settled_late')
        OR from_escrow_minor + from_wallet_minor + from_loss_minor = amount_minor
    )
);

CREATE INDEX offline_receipts_voucher ON offline_receipts (voucher_id, counter);

-- A receipt is recorded once and only its outcome moves forward: a settled
-- receipt never changes, and nothing it was signed with ever does.
CREATE FUNCTION mezgeb_offline_receipt_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'offline receipts are never deleted' USING ERRCODE = 'MZ011';
    END IF;
    IF OLD.outcome IN ('settled', 'settled_late') THEN
        RAISE EXCEPTION 'a settled receipt never changes' USING ERRCODE = 'MZ011';
    END IF;
    IF NEW.device_id <> OLD.device_id OR NEW.counter <> OLD.counter
       OR NEW.voucher_id <> OLD.voucher_id OR NEW.digest <> OLD.digest
       OR NEW.prev_digest <> OLD.prev_digest OR NEW.body <> OLD.body
       OR NEW.signature <> OLD.signature OR NEW.payee_account_id <> OLD.payee_account_id
       OR NEW.currency <> OLD.currency OR NEW.amount_minor <> OLD.amount_minor
       OR NEW.event_time <> OLD.event_time OR NEW.uploaded_by <> OLD.uploaded_by
       OR NEW.received_at <> OLD.received_at THEN
        RAISE EXCEPTION 'a receipt''s signed fields never change' USING ERRCODE = 'MZ011';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER offline_receipts_guard
    BEFORE UPDATE OR DELETE ON offline_receipts
    FOR EACH ROW EXECUTE FUNCTION mezgeb_offline_receipt_guard();

-- A second, different receipt for a counter already recorded: the evidence
-- of a cloned or rolled-back device (protocol §10). Append-only.
CREATE TABLE offline_receipt_conflicts (
    id          BIGINT      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    device_id   UUID        NOT NULL,
    counter     BIGINT      NOT NULL,
    digest      BYTEA       NOT NULL CHECK (length(digest) = 32),
    body        BYTEA       NOT NULL CHECK (length(body) <= 1024),
    signature   BYTEA       NOT NULL CHECK (length(signature) = 64),
    uploaded_by UUID        NOT NULL REFERENCES clients (id),
    received_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),

    FOREIGN KEY (device_id, counter) REFERENCES offline_receipts (device_id, counter),
    UNIQUE (device_id, counter, digest)
);

CREATE TRIGGER offline_receipt_conflicts_no_change
    BEFORE UPDATE OR DELETE ON offline_receipt_conflicts
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

-- A client's offline settings: the merchant fee on offline payments, in
-- basis points (E17 books 2% as 200). Without a row, no fee.
CREATE TABLE offline_settings (
    client_id  UUID        PRIMARY KEY REFERENCES clients (id),
    fee_bps    INTEGER     NOT NULL CHECK (fee_bps BETWEEN 0 AND 1000),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

GRANT SELECT, INSERT ON offline_receipts, offline_receipt_conflicts TO mezgeb_app;
GRANT UPDATE (outcome, entry_id, from_escrow_minor, from_wallet_minor, from_loss_minor,
              gap_skipped, settled_at)
    ON offline_receipts TO mezgeb_app;
GRANT SELECT ON offline_settings TO mezgeb_app;
GRANT SELECT ON offline_receipts, offline_receipt_conflicts, offline_settings
    TO mezgeb_readonly, mezgeb_verifier;
-- `mezgeb-cli offline fee` sets a client's fee.
GRANT SELECT, INSERT, UPDATE ON offline_settings TO mezgeb_admin;
