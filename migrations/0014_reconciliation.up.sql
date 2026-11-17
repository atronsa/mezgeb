-- Reconciliation (step 2.15, ADR-0020). Documented in
-- docs/db-schema.md#reconciliation.
--
-- Reconciliation reads the ledger and writes only these tables. A run is
-- recorded once, when it ends. A statement line is recorded once and only
-- its match is set, once. A break is recorded once and only its status
-- moves forward, once: from open to cleared (a later run matched it),
-- resolved (an entry resolved it) or explained (an operator's reason).

-- One import or reconciliation run, with what it found.
CREATE TABLE recon_runs (
    id             UUID        PRIMARY KEY,
    kind           TEXT        NOT NULL
                   CHECK (kind IN ('import', 'external', 'internal', 'offline')),
    -- The reconciled account (import, external) or the client whose cache
    -- was compared (internal).
    account_id     UUID        REFERENCES accounts (id),
    client_id      UUID        REFERENCES clients (id),
    -- Where the statement came from: its file and mapping (import).
    source         TEXT        NOT NULL DEFAULT '' CHECK (length(source) <= 300),
    -- The run's clock: the age of every item is measured at this time.
    as_of          TIMESTAMPTZ NOT NULL,
    items          INTEGER     NOT NULL DEFAULT 0 CHECK (items >= 0),
    matched        INTEGER     NOT NULL DEFAULT 0 CHECK (matched >= 0),
    pending        INTEGER     NOT NULL DEFAULT 0 CHECK (pending >= 0),
    breaks_new     INTEGER     NOT NULL DEFAULT 0 CHECK (breaks_new >= 0),
    breaks_cleared INTEGER     NOT NULL DEFAULT 0 CHECK (breaks_cleared >= 0),
    breaks_open    INTEGER     NOT NULL DEFAULT 0 CHECK (breaks_open >= 0),
    runtime_micros BIGINT      NOT NULL CHECK (runtime_micros >= 0),
    recorded_at    TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),

    CONSTRAINT recon_runs_scope CHECK (CASE kind
        WHEN 'import'   THEN account_id IS NOT NULL AND client_id IS NULL
        WHEN 'external' THEN account_id IS NOT NULL AND client_id IS NULL
        WHEN 'internal' THEN account_id IS NULL AND client_id IS NOT NULL
        ELSE account_id IS NULL AND client_id IS NULL END)
);

CREATE INDEX recon_runs_recorded ON recon_runs (recorded_at);

-- One bank statement line, identified by the reconciled account and the
-- bank's line ID (ADR-0020 decision 4), and the ledger line it matched.
CREATE TABLE recon_items (
    id             UUID        PRIMARY KEY,
    -- References to runs are checked at commit: a run is recorded last,
    -- once its counts are known, in the same transaction as its rows.
    import_run_id  UUID        NOT NULL REFERENCES recon_runs (id) DEFERRABLE INITIALLY DEFERRED,
    account_id     UUID        NOT NULL REFERENCES accounts (id),
    line_ref       TEXT        NOT NULL CHECK (length(line_ref) BETWEEN 1 AND 100),
    booking_date   DATE        NOT NULL,
    -- Signed, in minor units: positive is money into the account.
    amount_minor   BIGINT      NOT NULL CHECK (amount_minor <> 0),
    currency       TEXT        NOT NULL REFERENCES currencies (code),
    reference      TEXT        NOT NULL DEFAULT '' CHECK (length(reference) <= 200),
    -- SHA-256 of the line as imported: the same ID with another digest is a
    -- statement conflict.
    digest         BYTEA       NOT NULL CHECK (length(digest) = 32),
    -- The match, set once. Keys reference the entry, not the line, so that
    -- the journal's own guard is what refuses a TRUNCATE of its lines; the
    -- engine checks that the line exists.
    matched_entry_id UUID        REFERENCES journal_entries (id),
    matched_line_no  SMALLINT    CHECK (matched_line_no BETWEEN 0 AND 99),
    match_rule     TEXT        CHECK (match_rule IN ('exact', 'rule', 'resolution')),
    matched_run_id UUID        REFERENCES recon_runs (id) DEFERRABLE INITIALLY DEFERRED,

    UNIQUE (account_id, line_ref),
    -- A ledger line matches at most one statement line.
    UNIQUE (matched_entry_id, matched_line_no),
    -- Matched by a run (exact, rule) or by the entry that resolved its
    -- break (resolution); either way, to one ledger line.
    CONSTRAINT recon_items_match_whole CHECK (
        (match_rule IS NULL) = (matched_entry_id IS NULL)
        AND (matched_entry_id IS NULL) = (matched_line_no IS NULL)
        AND (match_rule IN ('exact', 'rule')) = (matched_run_id IS NOT NULL)
    )
);

CREATE INDEX recon_items_unmatched ON recon_items (account_id, booking_date)
    WHERE match_rule IS NULL;

-- One disagreement, open until it is cleared, resolved or explained. One
-- break is open per subject (ADR-0020 decision 6).
CREATE TABLE recon_breaks (
    id             UUID        PRIMARY KEY,
    run_id         UUID        NOT NULL REFERENCES recon_runs (id) DEFERRABLE INITIALLY DEFERRED,
    kind           TEXT        NOT NULL CHECK (kind IN (
                       'missing_in_ledger', 'missing_in_statement', 'amount_mismatch',
                       'duplicate_statement_line', 'statement_conflict', 'stale_cache',
                       'missing_receipts', 'counter_behind', 'receipt_entry_mismatch',
                       'escrow_mismatch')),
    -- What the break is about, stable across runs, such as `item:<id>`.
    subject        TEXT        NOT NULL CHECK (length(subject) BETWEEN 1 AND 200),
    account_id     UUID        REFERENCES accounts (id),
    client_id      UUID        REFERENCES clients (id),
    item_id        UUID        REFERENCES recon_items (id),
    entry_id       UUID        REFERENCES journal_entries (id),
    line_no        SMALLINT    CHECK (line_no BETWEEN 0 AND 99),
    device_id      UUID        REFERENCES devices (id),
    voucher_id     UUID        REFERENCES vouchers (id),
    -- What the other record says, what the ledger says, and the
    -- difference (other minus ledger), in minor units, where they apply.
    other_minor    BIGINT,
    ledger_minor   BIGINT,
    difference_minor BIGINT,
    detail         TEXT        NOT NULL DEFAULT '' CHECK (length(detail) <= 1000),
    found_at       TIMESTAMPTZ NOT NULL,
    status         TEXT        NOT NULL DEFAULT 'open'
                   CHECK (status IN ('open', 'cleared', 'resolved', 'explained')),
    -- How it closed: the run that cleared it, or the entry and its 1010
    -- line that resolved it, or the reason given.
    cleared_by_run_id   UUID   REFERENCES recon_runs (id) DEFERRABLE INITIALLY DEFERRED,
    resolution_entry_id UUID   UNIQUE REFERENCES journal_entries (id),
    resolution_line_no  SMALLINT CHECK (resolution_line_no BETWEEN 0 AND 99),
    resolution_note     TEXT   CHECK (length(resolution_note) BETWEEN 1 AND 1000),
    closed_by      UUID        REFERENCES clients (id),
    closed_at      TIMESTAMPTZ,

    CONSTRAINT recon_breaks_line_whole CHECK ((entry_id IS NULL) = (line_no IS NULL)),
    CONSTRAINT recon_breaks_closed_how CHECK (CASE status
        WHEN 'open' THEN closed_at IS NULL AND cleared_by_run_id IS NULL
                         AND resolution_entry_id IS NULL AND resolution_note IS NULL
        WHEN 'cleared' THEN closed_at IS NOT NULL AND cleared_by_run_id IS NOT NULL
        WHEN 'resolved' THEN closed_at IS NOT NULL AND resolution_entry_id IS NOT NULL
                             AND resolution_line_no IS NOT NULL
        ELSE closed_at IS NOT NULL AND resolution_note IS NOT NULL END)
);

CREATE UNIQUE INDEX recon_breaks_one_open ON recon_breaks (subject) WHERE status = 'open';
CREATE INDEX recon_breaks_line ON recon_breaks (entry_id, line_no) WHERE entry_id IS NOT NULL;
CREATE INDEX recon_breaks_resolution_line ON recon_breaks (resolution_entry_id, resolution_line_no)
    WHERE resolution_entry_id IS NOT NULL;
CREATE INDEX recon_breaks_item ON recon_breaks (item_id) WHERE item_id IS NOT NULL;
CREATE INDEX recon_breaks_account ON recon_breaks (account_id, status) WHERE account_id IS NOT NULL;
CREATE INDEX recon_breaks_kind ON recon_breaks (status, kind, found_at);

-- What a device reported as its last counter when it asked for this
-- voucher (step 2.11): the offline reconciliation compares it with the
-- receipts received. NULL for vouchers issued before step 2.15.
ALTER TABLE vouchers ADD COLUMN reported_last_counter BIGINT
    CHECK (reported_last_counter >= 0);

-- Runs never change. A statement line's match is set once, and nothing
-- else about it ever changes.
CREATE TRIGGER recon_runs_no_change
    BEFORE UPDATE OR DELETE ON recon_runs
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

CREATE FUNCTION mezgeb_recon_item_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'recon_items rows are never deleted' USING ERRCODE = 'MZ001';
    END IF;
    IF OLD.match_rule IS NOT NULL
       OR (NEW.id, NEW.import_run_id, NEW.account_id, NEW.line_ref, NEW.booking_date,
           NEW.amount_minor, NEW.currency, NEW.reference, NEW.digest)
          IS DISTINCT FROM
          (OLD.id, OLD.import_run_id, OLD.account_id, OLD.line_ref, OLD.booking_date,
           OLD.amount_minor, OLD.currency, OLD.reference, OLD.digest) THEN
        RAISE EXCEPTION 'statement line %: only an unmatched line''s match may be set', OLD.id
            USING ERRCODE = 'MZ001';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER recon_items_guard
    BEFORE UPDATE OR DELETE ON recon_items
    FOR EACH ROW EXECUTE FUNCTION mezgeb_recon_item_guard();

CREATE FUNCTION mezgeb_recon_break_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'recon_breaks rows are never deleted' USING ERRCODE = 'MZ001';
    END IF;
    IF OLD.status <> 'open'
       OR (NEW.id, NEW.run_id, NEW.kind, NEW.subject, NEW.account_id, NEW.client_id,
           NEW.item_id, NEW.entry_id, NEW.line_no, NEW.device_id, NEW.voucher_id,
           NEW.other_minor, NEW.ledger_minor, NEW.difference_minor, NEW.detail, NEW.found_at)
          IS DISTINCT FROM
          (OLD.id, OLD.run_id, OLD.kind, OLD.subject, OLD.account_id, OLD.client_id,
           OLD.item_id, OLD.entry_id, OLD.line_no, OLD.device_id, OLD.voucher_id,
           OLD.other_minor, OLD.ledger_minor, OLD.difference_minor, OLD.detail, OLD.found_at)
    THEN
        RAISE EXCEPTION 'break %: only an open break''s status may move', OLD.id
            USING ERRCODE = 'MZ001';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER recon_breaks_guard
    BEFORE UPDATE OR DELETE ON recon_breaks
    FOR EACH ROW EXECUTE FUNCTION mezgeb_recon_break_guard();

-- The service compares client caches and closes breaks; `mezgeb-cli recon`,
-- connected as mezgeb_verifier, imports statements and runs the daily
-- reconciliation. Neither may change the ledger through these grants.
GRANT SELECT, INSERT ON recon_runs, recon_items, recon_breaks TO mezgeb_app, mezgeb_verifier;
GRANT UPDATE (matched_entry_id, matched_line_no, match_rule, matched_run_id)
    ON recon_items TO mezgeb_app, mezgeb_verifier;
GRANT UPDATE (status, cleared_by_run_id, resolution_entry_id, resolution_line_no,
              resolution_note, closed_by, closed_at)
    ON recon_breaks TO mezgeb_app, mezgeb_verifier;
GRANT SELECT ON recon_runs, recon_items, recon_breaks TO mezgeb_readonly;
