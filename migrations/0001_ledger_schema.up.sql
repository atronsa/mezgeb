-- Ledger schema v1 (step 1.08). Documented in docs/db-schema.md.
--
-- The database enforces the core invariants on its own, so that a bug in the
-- engine, a manual SQL session or future code cannot break them. Each guard
-- names the invariant it protects (docs/invariants.md).
--
-- Errors raised by triggers use Mezgeb's own SQLSTATE codes:
--   MZ001  journal history changed (INV-9)
--   MZ002  entry unbalanced or with fewer than 2 lines (INV-1, INV-4)
--   MZ003  invalid reversal (INV-14)
--   MZ004  line posted to an account that is not open (INV-24)
--   MZ005  forbidden account change (immutable column, reopening, totals going down)

-- ---------------------------------------------------------------------------
-- Currencies (ISO 4217). The same table as crates/core/src/currency.rs.
-- ---------------------------------------------------------------------------

CREATE TABLE currencies (
    code         TEXT     PRIMARY KEY CHECK (code ~ '^[A-Z]{3}$'),
    numeric_code SMALLINT NOT NULL UNIQUE CHECK (numeric_code BETWEEN 1 AND 999),
    exponent     SMALLINT NOT NULL CHECK (exponent BETWEEN 0 AND 4)
);

INSERT INTO currencies (code, numeric_code, exponent) VALUES
    ('ETB', 230, 2),
    ('USD', 840, 2),
    ('EUR', 978, 2),
    ('GBP', 826, 2),
    ('KES', 404, 2),
    ('TZS', 834, 2),
    ('UGX', 800, 0),
    ('NGN', 566, 2),
    ('ZAR', 710, 2),
    ('JPY', 392, 0),
    ('KWD', 414, 3);

-- ---------------------------------------------------------------------------
-- Accounts
-- ---------------------------------------------------------------------------

CREATE TABLE accounts (
    id             UUID        PRIMARY KEY,
    code           SMALLINT    NOT NULL
                   CHECK (code BETWEEN 1000 AND 5999 OR code BETWEEN 9000 AND 9999),
    sub_account    TEXT        CHECK (sub_account ~ '^[a-z]{2,8}-[a-z0-9-]+$'
                                      AND length(sub_account) <= 64),
    name           TEXT        NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    account_type   TEXT        NOT NULL
                   CHECK (account_type IN ('asset', 'liability', 'equity', 'revenue', 'expense')),
    currency       TEXT        NOT NULL REFERENCES currencies (code),
    status         TEXT        NOT NULL DEFAULT 'open'
                   CHECK (status IN ('open', 'frozen', 'closed')),
    allow_negative BOOLEAN     NOT NULL DEFAULT false,
    debits_total   BIGINT      NOT NULL DEFAULT 0 CHECK (debits_total >= 0),
    credits_total  BIGINT      NOT NULL DEFAULT 0 CHECK (credits_total >= 0),
    version        BIGINT      NOT NULL DEFAULT 0 CHECK (version >= 0),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- One account per chart reference: "1010" or "2010:cust-abebe".
    CONSTRAINT accounts_reference_unique UNIQUE NULLS NOT DISTINCT (code, sub_account),
    -- Lets journal_lines check that a line's currency is its account's (INV-3).
    CONSTRAINT accounts_id_currency_unique UNIQUE (id, currency),
    -- INV-8: no balance below zero on the normal side unless allowed.
    CONSTRAINT accounts_not_negative CHECK (
        allow_negative
        OR CASE WHEN account_type IN ('asset', 'expense')
                THEN debits_total >= credits_total
                ELSE credits_total >= debits_total
           END
    ),
    -- A closed account has a zero balance.
    CONSTRAINT accounts_closed_at_zero CHECK (status <> 'closed' OR debits_total = credits_total)
);

-- Identity columns never change, a closed account never reopens, and
-- running totals never go down (they only ever add posted lines).
CREATE FUNCTION mezgeb_accounts_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id <> OLD.id
       OR NEW.code <> OLD.code
       OR NEW.sub_account IS DISTINCT FROM OLD.sub_account
       OR NEW.account_type <> OLD.account_type
       OR NEW.currency <> OLD.currency
       OR NEW.allow_negative <> OLD.allow_negative
       OR NEW.created_at <> OLD.created_at THEN
        RAISE EXCEPTION 'account %: identity columns cannot change', OLD.id
            USING ERRCODE = 'MZ005';
    END IF;
    IF OLD.status = 'closed' AND NEW.status <> 'closed' THEN
        RAISE EXCEPTION 'account %: a closed account cannot reopen', OLD.id
            USING ERRCODE = 'MZ005';
    END IF;
    IF NEW.debits_total < OLD.debits_total OR NEW.credits_total < OLD.credits_total THEN
        RAISE EXCEPTION 'account %: totals can only increase', OLD.id
            USING ERRCODE = 'MZ005';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER accounts_guard
    BEFORE UPDATE ON accounts
    FOR EACH ROW EXECUTE FUNCTION mezgeb_accounts_guard();

-- ---------------------------------------------------------------------------
-- Journal entries
-- ---------------------------------------------------------------------------

CREATE TABLE journal_entries (
    id                UUID        PRIMARY KEY,
    -- Posting order (INV-12).
    sequence          BIGINT      GENERATED ALWAYS AS IDENTITY UNIQUE,
    client_id         UUID        NOT NULL,
    idempotency_key   TEXT        NOT NULL CHECK (idempotency_key ~ '^[A-Za-z0-9._:-]{1,128}$'),
    -- INV-14: an entry is reversed at most once.
    reverses_entry_id UUID        UNIQUE REFERENCES journal_entries (id),
    metadata          JSONB       NOT NULL DEFAULT '{}'
                      CHECK (jsonb_typeof(metadata) = 'object'
                             AND octet_length(metadata::text) <= 8192),
    event_time        TIMESTAMPTZ,
    recorded_time     TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),

    -- INV-13: an idempotency key is used once per client.
    CONSTRAINT journal_entries_idempotency_unique UNIQUE (client_id, idempotency_key),
    CONSTRAINT journal_entries_not_self_reversal CHECK (reverses_entry_id <> id)
);

-- ---------------------------------------------------------------------------
-- Journal lines
-- ---------------------------------------------------------------------------

CREATE TABLE journal_lines (
    entry_id      UUID     NOT NULL REFERENCES journal_entries (id),
    line_no       SMALLINT NOT NULL CHECK (line_no BETWEEN 0 AND 99),
    account_id    UUID     NOT NULL,
    side          TEXT     NOT NULL CHECK (side IN ('debit', 'credit')),
    -- INV-1: every amount is greater than zero.
    amount        BIGINT   NOT NULL CHECK (amount > 0),
    currency      TEXT     NOT NULL REFERENCES currencies (code),
    -- The account's totals right after this line (ADR-0012, INV-6).
    debits_after  BIGINT   NOT NULL CHECK (debits_after >= 0),
    credits_after BIGINT   NOT NULL CHECK (credits_after >= 0),

    PRIMARY KEY (entry_id, line_no),
    -- INV-3: a line is in its account's currency.
    CONSTRAINT journal_lines_account_currency_fk
        FOREIGN KEY (account_id, currency) REFERENCES accounts (id, currency)
);

CREATE INDEX journal_lines_account_idx ON journal_lines (account_id);

-- INV-24: lines post only to open accounts.
CREATE FUNCTION mezgeb_check_line_account_open() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    account_status TEXT;
BEGIN
    SELECT status INTO account_status FROM accounts WHERE id = NEW.account_id;
    IF account_status <> 'open' THEN
        RAISE EXCEPTION 'entry % line %: account % is %',
            NEW.entry_id, NEW.line_no, NEW.account_id, account_status
            USING ERRCODE = 'MZ004';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER journal_lines_account_open
    BEFORE INSERT ON journal_lines
    FOR EACH ROW EXECUTE FUNCTION mezgeb_check_line_account_open();

-- ---------------------------------------------------------------------------
-- INV-1 and INV-4: checked at commit, once every line of the entry is in.
-- ---------------------------------------------------------------------------

CREATE FUNCTION mezgeb_check_entry_balanced() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    checked_id UUID;
    line_count INTEGER;
    unbalanced RECORD;
BEGIN
    IF TG_TABLE_NAME = 'journal_entries' THEN
        checked_id := NEW.id;
    ELSE
        checked_id := NEW.entry_id;
    END IF;

    SELECT count(*) INTO line_count FROM journal_lines WHERE entry_id = checked_id;
    IF line_count < 2 THEN
        RAISE EXCEPTION 'entry % has % line(s); at least 2 are required', checked_id, line_count
            USING ERRCODE = 'MZ002';
    END IF;

    SELECT currency,
           sum(amount) FILTER (WHERE side = 'debit')  AS debits,
           sum(amount) FILTER (WHERE side = 'credit') AS credits
      INTO unbalanced
      FROM journal_lines
     WHERE entry_id = checked_id
     GROUP BY currency
    HAVING coalesce(sum(amount) FILTER (WHERE side = 'debit'), 0)
        <> coalesce(sum(amount) FILTER (WHERE side = 'credit'), 0)
     LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'entry % is unbalanced in %: debits %, credits %',
            checked_id, unbalanced.currency,
            coalesce(unbalanced.debits, 0), coalesce(unbalanced.credits, 0)
            USING ERRCODE = 'MZ002';
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER journal_entries_balanced
    AFTER INSERT ON journal_entries
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION mezgeb_check_entry_balanced();

CREATE CONSTRAINT TRIGGER journal_lines_balanced
    AFTER INSERT ON journal_lines
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION mezgeb_check_entry_balanced();

-- ---------------------------------------------------------------------------
-- INV-14: a reversal never reverses a reversal, and mirrors its original.
-- ---------------------------------------------------------------------------

CREATE FUNCTION mezgeb_check_reversal_target() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM journal_entries
                WHERE id = NEW.reverses_entry_id AND reverses_entry_id IS NOT NULL) THEN
        RAISE EXCEPTION 'entry % is a reversal and cannot be reversed', NEW.reverses_entry_id
            USING ERRCODE = 'MZ003';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER journal_entries_reversal_target
    BEFORE INSERT ON journal_entries
    FOR EACH ROW WHEN (NEW.reverses_entry_id IS NOT NULL)
    EXECUTE FUNCTION mezgeb_check_reversal_target();

CREATE FUNCTION mezgeb_check_reversal_mirror() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        (SELECT line_no, account_id, side, amount, currency
           FROM journal_lines WHERE entry_id = NEW.reverses_entry_id
         EXCEPT
         SELECT line_no, account_id,
                CASE side WHEN 'debit' THEN 'credit' ELSE 'debit' END, amount, currency
           FROM journal_lines WHERE entry_id = NEW.id)
        UNION ALL
        (SELECT line_no, account_id,
                CASE side WHEN 'debit' THEN 'credit' ELSE 'debit' END, amount, currency
           FROM journal_lines WHERE entry_id = NEW.id
         EXCEPT
         SELECT line_no, account_id, side, amount, currency
           FROM journal_lines WHERE entry_id = NEW.reverses_entry_id)
    ) THEN
        RAISE EXCEPTION 'reversal % does not mirror entry %', NEW.id, NEW.reverses_entry_id
            USING ERRCODE = 'MZ003';
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER journal_entries_reversal_mirror
    AFTER INSERT ON journal_entries
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW WHEN (NEW.reverses_entry_id IS NOT NULL)
    EXECUTE FUNCTION mezgeb_check_reversal_mirror();

-- ---------------------------------------------------------------------------
-- INV-9: history is append-only. Blocked here by triggers, for everyone, and
-- for the service again by its role having no UPDATE or DELETE privilege.
-- ---------------------------------------------------------------------------

CREATE FUNCTION mezgeb_reject_change() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION '% on % is not allowed: the journal is append-only', TG_OP, TG_TABLE_NAME
        USING ERRCODE = 'MZ001';
END
$$;

CREATE TRIGGER journal_entries_append_only
    BEFORE UPDATE OR DELETE ON journal_entries
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

CREATE TRIGGER journal_entries_no_truncate
    BEFORE TRUNCATE ON journal_entries
    FOR EACH STATEMENT EXECUTE FUNCTION mezgeb_reject_change();

CREATE TRIGGER journal_lines_append_only
    BEFORE UPDATE OR DELETE ON journal_lines
    FOR EACH ROW EXECUTE FUNCTION mezgeb_reject_change();

CREATE TRIGGER journal_lines_no_truncate
    BEFORE TRUNCATE ON journal_lines
    FOR EACH STATEMENT EXECUTE FUNCTION mezgeb_reject_change();

-- ---------------------------------------------------------------------------
-- Privileges (roles are created in db/init/01-roles.sql)
-- ---------------------------------------------------------------------------

GRANT SELECT ON currencies, accounts, journal_entries, journal_lines
    TO mezgeb_app, mezgeb_readonly;

GRANT INSERT ON accounts, journal_entries, journal_lines TO mezgeb_app;

-- The service may change only an account's totals, version and status.
GRANT UPDATE (debits_total, credits_total, version, status) ON accounts TO mezgeb_app;
