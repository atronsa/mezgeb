-- Balance verification (step 1.13). Documented in docs/db-schema.md.
--
-- Every verification run is recorded. A clean run also stores each
-- account's totals at its checkpoint (the highest entry sequence it
-- verified), so the next incremental run only has to check what was posted
-- after it.

CREATE TABLE verification_runs (
    id            BIGINT      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    scope         TEXT        NOT NULL CHECK (scope IN ('full', 'incremental')),
    -- Entries with from_sequence < sequence <= to_sequence were verified.
    from_sequence BIGINT      NOT NULL CHECK (from_sequence >= 0),
    to_sequence   BIGINT      NOT NULL CHECK (to_sequence >= from_sequence),
    started_at    TIMESTAMPTZ NOT NULL,
    finished_at   TIMESTAMPTZ NOT NULL,
    duration_ms   BIGINT      NOT NULL CHECK (duration_ms >= 0),
    lines_checked BIGINT      NOT NULL CHECK (lines_checked >= 0),
    result        TEXT        NOT NULL CHECK (result IN ('clean', 'dirty')),
    findings      JSONB       NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(findings) = 'array')
);

-- Each account's totals as of a clean run's to_sequence.
CREATE TABLE verification_checkpoints (
    run_id        BIGINT NOT NULL REFERENCES verification_runs (id),
    account_id    UUID   NOT NULL,
    debits_total  BIGINT NOT NULL CHECK (debits_total >= 0),
    credits_total BIGINT NOT NULL CHECK (credits_total >= 0),
    PRIMARY KEY (run_id, account_id)
);

-- A record of what was verified never changes.
CREATE TRIGGER verification_runs_append_only
    BEFORE UPDATE OR DELETE ON verification_runs
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

CREATE TRIGGER verification_checkpoints_append_only
    BEFORE UPDATE OR DELETE ON verification_checkpoints
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

-- The verifier reads the ledger and records its runs; it can change nothing
-- else (separation of duties, threat TM-15). The role is created in
-- db/init/02-verifier.sql.
GRANT SELECT ON currencies, accounts, journal_entries, journal_lines, idempotency_records,
    reason_codes TO mezgeb_verifier;
GRANT SELECT, INSERT ON verification_runs, verification_checkpoints TO mezgeb_verifier;
GRANT SELECT ON verification_runs, verification_checkpoints TO mezgeb_readonly;
