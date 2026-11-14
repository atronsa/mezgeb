-- Refunds (step 2.04, INV-15, ADR-0015). Documented in docs/db-schema.md.
--
-- A refund is a new entry that names the standard entry it refunds. It may
-- credit only accounts the original debited, and all of an entry's refunds
-- together move each account of the original back by at most what the
-- original moved it. The engine checks this with the original's refund
-- tallies locked, so concurrent refunds of one entry queue; the database
-- checks it again at commit.
--
-- New SQLSTATE:
--   MZ007  invalid refund, or a reversal of a refund or of a refunded entry

ALTER TABLE journal_entries
    ADD COLUMN refunds_entry_id UUID REFERENCES journal_entries (id),
    DROP CONSTRAINT journal_entries_kind_check,
    ADD CONSTRAINT journal_entries_kind_check
        CHECK (kind IN ('standard', 'reversal', 'adjustment', 'refund')),
    DROP CONSTRAINT journal_entries_authorization;

CREATE INDEX journal_entries_refunds_idx
    ON journal_entries (refunds_entry_id) WHERE refunds_entry_id IS NOT NULL;

-- The rule from 0003, plus refunds: only a refund names a refunded entry,
-- and a refund carries no reason or approver. NOT VALID, as in 0003.
ALTER TABLE journal_entries
    ADD CONSTRAINT journal_entries_authorization CHECK (
        CASE kind
            WHEN 'standard' THEN
                reverses_entry_id IS NULL AND refunds_entry_id IS NULL
                AND reason_code IS NULL AND requested_by IS NULL AND approved_by IS NULL
            WHEN 'reversal' THEN
                reverses_entry_id IS NOT NULL AND refunds_entry_id IS NULL
                AND reason_code IS NOT NULL AND requested_by IS NOT NULL
                AND approved_by IS DISTINCT FROM requested_by
            WHEN 'adjustment' THEN
                reverses_entry_id IS NULL AND refunds_entry_id IS NULL
                AND reason_code IS NOT NULL AND requested_by IS NOT NULL
                AND approved_by IS NOT NULL AND approved_by <> requested_by
            WHEN 'refund' THEN
                reverses_entry_id IS NULL AND refunds_entry_id IS NOT NULL
                AND refunds_entry_id <> id
                AND reason_code IS NULL AND requested_by IS NULL AND approved_by IS NULL
            ELSE false
        END
    ) NOT VALID;

-- What the refunds of each entry have moved back, per account of the
-- original, and the most they may. The engine locks an entry's rows before
-- refunding or reversing it.
CREATE TABLE refund_tallies (
    entry_id     UUID   NOT NULL REFERENCES journal_entries (id),
    account_id   UUID   NOT NULL REFERENCES accounts (id),
    limit_amount BIGINT NOT NULL CHECK (limit_amount > 0),
    refunded     BIGINT NOT NULL DEFAULT 0 CHECK (refunded >= 0),

    PRIMARY KEY (entry_id, account_id),
    -- INV-15: never more back than the original moved.
    CONSTRAINT refund_tallies_within_limit CHECK (refunded <= limit_amount)
);

-- A tally only grows, and only its refunded amount changes.
CREATE FUNCTION mezgeb_refund_tallies_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.entry_id <> OLD.entry_id
       OR NEW.account_id <> OLD.account_id
       OR NEW.limit_amount <> OLD.limit_amount
       OR NEW.refunded < OLD.refunded THEN
        RAISE EXCEPTION 'refund tally of % for %: only growing the refunded amount is allowed',
            OLD.entry_id, OLD.account_id
            USING ERRCODE = 'MZ007';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER refund_tallies_guard
    BEFORE UPDATE ON refund_tallies
    FOR EACH ROW EXECUTE FUNCTION mezgeb_refund_tallies_guard();

CREATE TRIGGER refund_tallies_no_delete
    BEFORE DELETE ON refund_tallies
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

-- INV-15 again, from the journal alone, at commit: a refund names an
-- unreversed standard entry, every refund line moves an account the way
-- the rule allows, and all the original's refunds together stay within it.
-- A reversal of a refund, or of an entry with refunds, is refused.
CREATE FUNCTION mezgeb_check_refund() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    original journal_entries%ROWTYPE;
    broken   UUID;
BEGIN
    IF NEW.kind = 'reversal' THEN
        IF EXISTS (SELECT 1 FROM journal_entries
                    WHERE id = NEW.reverses_entry_id AND kind = 'refund')
           OR EXISTS (SELECT 1 FROM journal_entries
                       WHERE refunds_entry_id = NEW.reverses_entry_id) THEN
            RAISE EXCEPTION 'entry % is a refund or has refunds and cannot be reversed',
                NEW.reverses_entry_id
                USING ERRCODE = 'MZ007';
        END IF;
        RETURN NULL;
    END IF;
    IF NEW.kind <> 'refund' THEN
        RETURN NULL;
    END IF;

    SELECT * INTO original FROM journal_entries WHERE id = NEW.refunds_entry_id;
    IF original.kind IS DISTINCT FROM 'standard'
       OR EXISTS (SELECT 1 FROM journal_entries WHERE reverses_entry_id = original.id) THEN
        RAISE EXCEPTION 'refund % refunds %, which is not an unreversed standard entry',
            NEW.id, NEW.refunds_entry_id
            USING ERRCODE = 'MZ007';
    END IF;

    WITH net AS (
        SELECT account_id,
               sum(CASE side WHEN 'debit' THEN amount ELSE -amount END) AS net
          FROM journal_lines WHERE entry_id = original.id
         GROUP BY account_id
    ), back AS (
        SELECT l.account_id, l.side, l.amount
          FROM journal_lines l
          JOIN journal_entries e ON e.id = l.entry_id
         WHERE e.refunds_entry_id = original.id
    )
    SELECT account_id INTO broken FROM (
        -- A line the wrong way: crediting an account the original did not
        -- debit, or debiting one it debited.
        SELECT b.account_id
          FROM back b LEFT JOIN net n USING (account_id)
         WHERE (coalesce(n.net, 0) > 0 AND b.side = 'debit')
            OR (coalesce(n.net, 0) <= 0 AND b.side = 'credit')
        UNION ALL
        -- More back than the original moved.
        SELECT n.account_id
          FROM net n JOIN back b USING (account_id)
         GROUP BY n.account_id, n.net
        HAVING sum(b.amount) > abs(n.net)
    ) violations
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'the refunds of % move account % back more than allowed',
            original.id, broken
            USING ERRCODE = 'MZ007';
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER journal_entries_refund_rules
    AFTER INSERT ON journal_entries
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION mezgeb_check_refund();

GRANT SELECT, INSERT ON refund_tallies TO mezgeb_app;
GRANT UPDATE (refunded) ON refund_tallies TO mezgeb_app;
GRANT SELECT ON refund_tallies TO mezgeb_readonly, mezgeb_verifier;
