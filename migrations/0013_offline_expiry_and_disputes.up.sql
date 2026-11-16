-- Offline gaps, expiry, late receipts and disputes (step 2.13, ADR-0019,
-- docs/offline/protocol.md §7, §11, §12, §14). Documented in
-- docs/db-schema.md#offline-expiry-late-receipts-and-disputes.

-- A voucher's end of life: when its escrow went back, by which entry, and
-- when late receipts stopped being accepted.
ALTER TABLE vouchers
    ADD COLUMN returned_at     TIMESTAMPTZ,
    ADD COLUMN return_entry_id UUID UNIQUE REFERENCES journal_entries (id),
    ADD COLUMN ended_at        TIMESTAMPTZ,
    ADD CONSTRAINT vouchers_returned_when CHECK (
        (status IN ('returned', 'closed', 'ended')) = (returned_at IS NOT NULL)
    ),
    ADD CONSTRAINT vouchers_ended_when CHECK ((status = 'ended') = (ended_at IS NOT NULL));

-- How long after the return a late receipt is still paid (protocol §12):
-- 90 days unless the client sets otherwise, at most 365.
ALTER TABLE offline_settings
    ADD COLUMN late_deadline_days INTEGER NOT NULL DEFAULT 90
        CHECK (late_deadline_days BETWEEN 1 AND 365);
ALTER TABLE offline_settings ALTER COLUMN fee_bps SET DEFAULT 0;

-- A receipt the operator refused at the end of a dispute: its payee bears it.
ALTER TABLE offline_receipts DROP CONSTRAINT offline_receipts_outcome_check;
ALTER TABLE offline_receipts ADD CONSTRAINT offline_receipts_outcome_check CHECK (outcome IN (
    'settled', 'settled_late', 'waiting_for_gap', 'conflict', 'frozen', 'over_limit',
    'expired', 'refused'));

-- What a late receipt's payer could not pay, paid by the loss account 5030
-- (E29). Until it is repaid (E30), no device of the payer gets a voucher.
CREATE TABLE offline_shortfalls (
    id               UUID        PRIMARY KEY,
    client_id        UUID        NOT NULL REFERENCES clients (id),
    -- The payer's wallet.
    owner_account_id UUID        NOT NULL REFERENCES accounts (id),
    device_id        UUID        NOT NULL REFERENCES devices (id),
    counter          BIGINT      NOT NULL,
    currency         TEXT        NOT NULL REFERENCES currencies (code),
    amount_minor     BIGINT      NOT NULL CHECK (amount_minor > 0),
    -- The entry that paid it from 5030, and the one that repaid it.
    entry_id         UUID        NOT NULL REFERENCES journal_entries (id),
    repaid_entry_id  UUID        UNIQUE REFERENCES journal_entries (id),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    repaid_at        TIMESTAMPTZ,

    CONSTRAINT offline_shortfalls_repaid CHECK ((repaid_entry_id IS NULL) = (repaid_at IS NULL))
);

CREATE INDEX offline_shortfalls_unpaid ON offline_shortfalls (owner_account_id)
    WHERE repaid_entry_id IS NULL;

-- A frozen device's case (protocol §14). Opened by the freeze, resolved by
-- an operator: `paid` pays every valid receipt under the loss policy,
-- `refused` pays none. Either way the escrow left returns and the device is
-- revoked.
CREATE TABLE offline_disputes (
    id          UUID        PRIMARY KEY,
    client_id   UUID        NOT NULL REFERENCES clients (id),
    device_id   UUID        NOT NULL REFERENCES devices (id),
    voucher_id  UUID        NOT NULL REFERENCES vouchers (id),
    -- Why it froze, and at which counter.
    reason      TEXT        NOT NULL CHECK (reason IN ('conflict', 'over_limit')),
    counter     BIGINT      NOT NULL,
    status      TEXT        NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved')),
    resolution  TEXT        CHECK (resolution IN ('paid', 'refused')),
    resolved_by TEXT        CHECK (length(resolved_by) <= 128),
    opened_at   TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    resolved_at TIMESTAMPTZ,

    CONSTRAINT offline_disputes_resolved CHECK (
        (status = 'resolved') = (resolution IS NOT NULL AND resolved_by IS NOT NULL
                                 AND resolved_at IS NOT NULL)
    )
);

-- One open case per voucher.
CREATE UNIQUE INDEX offline_disputes_one_open ON offline_disputes (voucher_id)
    WHERE status = 'open';

-- How a dispute paid a conflicting receipt, which has no row of its own in
-- offline_receipts (E31).
CREATE TABLE offline_dispute_payments (
    dispute_id        UUID        NOT NULL REFERENCES offline_disputes (id),
    conflict_id       BIGINT      NOT NULL UNIQUE REFERENCES offline_receipt_conflicts (id),
    entry_id          UUID        NOT NULL UNIQUE REFERENCES journal_entries (id),
    from_escrow_minor BIGINT      NOT NULL CHECK (from_escrow_minor >= 0),
    from_wallet_minor BIGINT      NOT NULL CHECK (from_wallet_minor >= 0),
    from_loss_minor   BIGINT      NOT NULL CHECK (from_loss_minor >= 0),
    paid_at           TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),

    PRIMARY KEY (dispute_id, conflict_id)
);

CREATE TRIGGER offline_dispute_payments_no_change
    BEFORE UPDATE OR DELETE ON offline_dispute_payments
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

GRANT UPDATE (returned_at, return_entry_id, ended_at) ON vouchers TO mezgeb_app;
GRANT SELECT, INSERT ON offline_shortfalls, offline_disputes, offline_dispute_payments
    TO mezgeb_app;
GRANT UPDATE (repaid_entry_id, repaid_at) ON offline_shortfalls TO mezgeb_app;
GRANT UPDATE (status, resolution, resolved_by, resolved_at) ON offline_disputes TO mezgeb_app;
GRANT SELECT ON offline_shortfalls, offline_disputes, offline_dispute_payments
    TO mezgeb_readonly, mezgeb_verifier;
