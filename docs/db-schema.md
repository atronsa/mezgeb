# Database Schema v1

[Documentation](README.md) · Source: [`migrations/0001_ledger_schema.up.sql`](../migrations/0001_ledger_schema.up.sql) · Step: [1.08](roadmap/phase-1-core-engine/08-postgresql-schema.md)

The database is the last line of defence. Rust checks every rule first
(`crates/core`), but a bug in the engine, a manual SQL session or future code
can bypass Rust. So the database enforces the core invariants again, on its
own, with constraints, triggers and role privileges.

## Tables

| Table | One row per | Notes |
|---|---|---|
| `currencies` | ISO 4217 currency | The same 11 currencies as `crates/core/src/currency.rs`, with their exponent |
| `accounts` | Account | Chart code and sub-account, type, currency, status, `allow_negative`, running totals (`debits_total`, `credits_total`), `version` |
| `journal_entries` | Posted entry | `sequence` (posting order), client ID and idempotency key, `reverses_entry_id`, metadata, `event_time`, `recorded_time` |
| `journal_lines` | Line of an entry | Account, side, amount, currency, and the account's totals right after the line (`debits_after`, `credits_after`) |
| `idempotency_records` | Successfully posted request | Client, idempotency key, SHA-256 fingerprint of the request, the entry it created (migration 0002, step 1.11) |
| `reason_codes` | Reason a correction may give | Code, the kind of entry it is for (reversal or adjustment), description; the same list as `REASON_CODES` in `crates/core` (migration 0003, step 1.12) |
| `verification_runs` | Verification run | Scope (full or incremental), the entries it covered (`from_sequence` < sequence ≤ `to_sequence`), duration, lines checked, result and findings (migration 0004, step 1.13) |
| `verification_checkpoints` | Account at a clean run's checkpoint | Each account's totals as of that run's `to_sequence`, the starting point for the next incremental run |
| `holds` | Hold | The account it holds from, its own held account, amount and currency, `captured`, status (`active`, `captured`, `released`, `expired`), `expires_at`, the entry that placed it and the entry that ended it (migration 0005, step 1.14) |
| `periods` | Accounting period | Label, status (`open`, `closing`, `closed`), the entry range it holds (`after_sequence` < sequence ≤ `last_sequence`), when it opened, stopped taking postings and closed (migration 0007, step 2.05) |
| `period_snapshots` | Account at the end of a closed period | The account's totals from every line up to the period's `last_sequence`; append-only |
| `refund_tallies` | Account of a refunded entry | What the entry's refunds have moved back on that account (`refunded`) and the most they may (`limit_amount`); it only grows (migration 0006, step 2.04) |

Since migration 0003, every journal entry also has a `kind` (`standard`,
`reversal`, `adjustment` or, since migration 0006, `refund`), and
corrections record `reason_code`, `requested_by` and `approved_by`. A
refund records the entry it refunds in `refunds_entry_id`. Since migration
0007, every new entry records its accounting period in `period_id`.

Amounts are `BIGINT` minor units ([ADR-0005](adr/0005-money-as-i64-minor-units.md)).
IDs are `UUID` ([ADR-0006](adr/0006-uuidv7-identifiers.md)). Status, type and
side are `TEXT` with a `CHECK`, not PostgreSQL enums, because adding a value to
an enum later is harder to migrate.

## Constraints and the invariants they protect

This is the step 1.08 exit gate: every constraint names its invariant
([invariant catalogue](invariants.md)).

| Constraint | Kind | Protects | Test (`crates/engine/tests/schema.rs`) |
|---|---|---|---|
| `journal_lines.amount > 0` | CHECK | INV-1: amounts are positive | `line_amounts_must_be_positive` |
| `journal_entries_balanced`, `journal_lines_balanced` → `mezgeb_check_entry_balanced()` | Deferred constraint trigger | INV-1: at least 2 lines, debits = credits per currency, checked at commit. INV-4: an entry without lines cannot commit | `unbalanced_entries_fail_at_commit` |
| `journal_lines.entry_id` → `journal_entries` | Foreign key | INV-4: no line without its entry | `lines_reference_their_entry_and_account_currency` |
| `journal_lines_account_currency_fk` → `accounts (id, currency)` | Foreign key | INV-3: a line is in its account's currency | `lines_reference_their_entry_and_account_currency` |
| `accounts_not_negative` | CHECK | INV-8: no balance below zero on the normal side unless `allow_negative` | `a_wallet_cannot_go_negative` |
| `accounts.debits_total >= 0`, `credits_total >= 0` | CHECK | INV-5, INV-8: totals stay in range | `a_wallet_cannot_go_negative` |
| `accounts_guard` → `mezgeb_accounts_guard()`: totals never decrease | Trigger | INV-5, INV-9: totals only ever add posted lines | `totals_only_increase` |
| `journal_entries_append_only`, `journal_lines_append_only`, `*_no_truncate` → `mezgeb_reject_change()` | Triggers | INV-9: history is never updated, deleted or truncated, even by the owner | `journal_history_cannot_change` |
| Role privileges: `mezgeb_app` has no UPDATE, DELETE or TRUNCATE on the journal | Privileges | INV-9, second layer | `journal_history_cannot_change` |
| `journal_entries_idempotency_unique (client_id, idempotency_key)` | UNIQUE | INV-13: an idempotency key is used once per client | `idempotency_keys_are_unique_per_client` |
| `idempotency_records` primary key `(client_id, idempotency_key)` | PRIMARY KEY | INV-13: claimed before any account is locked; a concurrent request with the same key waits on it | `fifty_concurrent_repeats_post_once` (`tests/idempotency.rs`) |
| `idempotency_records_entry_fk` → `journal_entries (id, client_id, idempotency_key)`, deferred | Foreign key | INV-13: a record and its entry always share client and key | `ten_sequential_repeats_post_once` |
| `idempotency_records_append_only` → `mezgeb_reject_change()`; `mezgeb_app` has no UPDATE or DELETE | Trigger, privileges | INV-9, INV-13: a record never changes | `idempotency_records_are_append_only` |
| `journal_entries_authorization` | CHECK | INV-14, INV-25: a standard entry has no reason or actors; a reversal has a reason and a requester (and an approver, if any, is someone else); an adjustment has a reason, a requester and a **different** approver | `adjustments_need_two_different_people` (`tests/corrections.rs`) |
| `journal_entries_reason_fk` → `reason_codes (code, entry_kind)` | Foreign key | A reason exists and is meant for this kind of entry | `adjustments_need_two_different_people` |
| `journal_entries.reverses_entry_id` UNIQUE | UNIQUE | INV-14: an entry is reversed at most once | `reversal_rules` |
| `journal_entries_reversal_target` → `mezgeb_check_reversal_target()` | Trigger | INV-14: a reversal is never reversed | `reversal_rules` |
| `journal_entries_reversal_mirror` → `mezgeb_check_reversal_mirror()` | Deferred constraint trigger | INV-14: a reversal mirrors its original exactly | `reversal_rules` |
| `journal_lines_account_open` → `mezgeb_check_line_account_open()` | Trigger | INV-24: lines post only to open accounts | `account_status_rules` |
| `accounts_closed_at_zero` | CHECK | Account lifecycle: an account closes only at zero balance | `account_status_rules` |
| `accounts_guard`: a closed account never reopens; identity columns never change | Trigger | Account lifecycle; INV-8 (`allow_negative` cannot be switched on later) | `account_status_rules`, `the_app_role_cannot_change_account_identity` |
| Role privileges: `mezgeb_app` may update only totals, `version` and `status` | Column privileges | Account identity cannot be changed by the service | `the_app_role_cannot_change_account_identity` |
| `journal_entries.sequence` identity | Identity column | INV-12: posting order | `a_valid_posting_commits` |
| `accounts_reference_unique (code, sub_account)`, `NULLS NOT DISTINCT` | UNIQUE | One account per chart reference | covered by the account helpers |
| Format CHECKs on codes, sub-accounts, idempotency keys, metadata | CHECK | Validation repeated from `crates/core` | `idempotency_keys_are_unique_per_client` |
| `holds_guard` → `mezgeb_holds_guard()` | Trigger | INV-16: a hold's status changes only from `active`, once; only status, `captured` and the closing entry may change | `the_database_guards_holds` (`tests/holds.rs`) |
| `holds_closed_by_entry`: active ⇔ no closing entry | CHECK | INV-16: every ended hold names the entry that ended it | `the_database_guards_holds` |
| `holds.captured` between 0 and `amount`; `holds_captured_only_when_captured` | CHECK | INV-17: a capture never exceeds the hold | `the_database_guards_holds` |
| `holds.held_account_id` UNIQUE; `holds_distinct_accounts` | UNIQUE, CHECK | INV-18: each hold has its own held account, never the account it holds from | `hold_operations_are_idempotent`, `held_accounts_are_protected_on_postgresql` |
| `holds.place_entry_id`, `close_entry_id` UNIQUE → `journal_entries` | UNIQUE, foreign key | A hold's entries are real, and each belongs to one hold | `the_database_guards_holds` |
| `holds_no_delete` → `mezgeb_reject_change()`; `mezgeb_app` may update only status, `captured` and `close_entry_id` | Trigger, column privileges | INV-9, INV-16: holds are never deleted, and their terms never change | `the_database_guards_holds` |

### Invariants marked "DB" that later steps add

Their tables do not exist yet. Each step adds the constraint and its test.

| Invariant | What the database will enforce | Step |
|---|---|---|
| INV-10 | No entry in a closed accounting period | 2.05 |
| INV-12 | `recorded_time` strictly increasing under concurrency: measured in step 1.10 (no tie or inversion in 1,000 parallel postings), guaranteed by the hash chain's single commit order | 2.09 |
| INV-19, INV-20 | Voucher escrow cap; one settlement per `(device_id, counter)` | 2.12 |
| INV-26 | One signed ledger receipt per entry | 2.09 |

## Idempotency and retention

How a posting uses its key (ADR-0008, step 1.11):

1. In the posting's transaction, before any account is locked, the key is
   claimed with `INSERT … ON CONFLICT DO NOTHING` on `idempotency_records`.
2. If the key was free, the posting goes ahead; the record commits with the
   entry, or both roll back. A refused posting therefore never uses up its
   key.
3. If the key is taken, the stored fingerprint is compared with the
   request's: the same fingerprint returns the original entry, a different
   one is refused (`IdempotencyKeyReused`). Nothing is written either way.
4. If another transaction holds the key and has not finished, PostgreSQL
   makes this one wait on the primary key, then applies step 3.

The response is rebuilt from the stored entry. Entries are immutable
(INV-9), so it is always identical; no separate copy is kept that could
drift.

**Retention.** Records are kept as long as their entry, which is forever:
a key stays unique per client permanently, so a late retry, however late,
can never post twice. That costs about 100 bytes per entry. Archival of old
records, if storage ever requires it, is decided with the journal's own
archival (step 3.04), and must keep keys unique for at least the longest
client retry window.

## Verification

`mezgeb-cli verify` (or `mezgeb_engine::verify`) proves that the stored
balances equal the journal (step 1.13, ADR-0012). In one consistent snapshot
it checks:

| Check | Invariant | Reported with |
|---|---|---|
| Each entry balances per currency | INV-1 | the entry |
| Each entry has at least two lines; no line lacks its entry | INV-1, INV-4 | the entry, or the entry and line |
| Each line's stored totals continue its account's chain | INV-6 | the account, and the first line where the chain breaks |
| Each account's totals equal its checkpoint plus every line after it | INV-5 | the account, stored and expected totals |
| All debits equal all credits per currency | INV-2 | the currency |
| Each held account holds its active hold's amount, or nothing once the hold has ended | INV-18 | the hold and its held account |

**Full and incremental.** A full run checks every entry. An incremental run
checks the first three only for entries after the last clean run's
checkpoint, and the last two for every account. A clean run stores each
account's totals at its checkpoint; a dirty run does not, so the next run
starts at the same place. An incremental run does not re-check lines it has
already verified: tampering with an old line is found by a full run (run
one regularly) and, from step 2.09, by the hash chain.

**No entry is skipped.** A run checkpoints only entries recorded at least
60 seconds before it starts (`--settle-seconds`), and the service role
`mezgeb_app` has `transaction_timeout = 30s`. An entry's sequence number is
taken when it is inserted, so any entry with a lower sequence was inserted
earlier, and its transaction has finished, committed or rolled back, by the
time the checkpoint is taken. No entry can commit late below a checkpoint.

**Recorded.** Every run is a row in `verification_runs`, with its findings;
both verification tables are append-only.

## Holds

A hold reserves money by a real posting into its own held account, such as
`2020:wd-0001` ([ADR-0004](adr/0004-holds-as-postings.md), step 1.14). It
then ends exactly once:

| Operation | Entry | Status |
|---|---|---|
| Place | account → held account | `active` |
| Capture (full or partial) | held account → destination, and the remainder back to the account, in one entry | `captured` |
| Release | held account → account | `released` |
| Expire (once `expires_at` has passed) | held account → account | `expired` |

Each operation is one transaction that locks the hold's row
(`SELECT … FOR UPDATE`), is idempotent like any posting, and tags its entry
with the metadata `hold` and `hold_action`. Ordinary postings, adjustments
and reversals that touch a held account are refused (`AccountIsHeld`):
only the hold's own entries move money in and out of it.

**The expiry worker** (`expire_due_holds`) takes due holds oldest first
from the partial index `holds_due_idx`, claiming each with
`FOR UPDATE SKIP LOCKED` and expiring it in its own transaction under the
idempotency key `hold-expire:<hold id>`.

- **Several workers can run at once.** A hold that a capture or release has
  locked is skipped, not waited for. Once that commits the hold is no longer
  active; if it rolls back, the hold is due again on the next run.
- **A capture racing an expiry has exactly one winner.** Whichever locks the
  row first ends the hold; the other finds it no longer active.
- **A worker that dies mid-batch is simply run again.** What it committed is
  done, and the rest is still due. The key, the entry and the status change
  commit together, so no hold is expired twice.

## Refunds

A refund names the standard entry it refunds (`refunds_entry_id`) and is
capped by it, account by account (INV-15,
[ADR-0015](adr/0015-refunds-and-disputes.md), step 2.04):

1. **The engine locks the original's tallies.** On a refund, it creates the
   original's `refund_tallies` rows if needed (`INSERT … ON CONFLICT DO
   NOTHING`, one per account, with the limit taken from the original's
   lines) and locks them, in account order, with `FOR UPDATE`. Concurrent
   refunds of one entry wait here, so each sees what the one before it gave
   back. Refunds of different entries do not touch each other's rows.
2. **It checks the refund** with `RefundLimits`, the same rule the reference
   ledger uses, after re-reading whether the original was reversed while it
   waited.
3. **It posts the refund and updates the tallies** in the same transaction.

A reversal takes the same tally locks before it checks that the entry has
no refunds, so a refund and a reversal of one entry never both succeed.

The database guards the rule on its own: `refund_tallies` refuses a
`refunded` above `limit_amount` (`23514`), a tally that shrinks or changes
anything else (`MZ007`), and any delete. At commit, the deferred trigger
`journal_entries_refund_rules` checks every refund again from the journal
alone (it names an unreversed standard entry, its lines move each account
the allowed way, all refunds of the original stay within it), and refuses a
reversal of a refund or of a refunded entry (`MZ007`).

## Periods

Every entry belongs to the period that was open when it was posted
(INV-10, [ADR-0016](adr/0016-accounting-periods.md), step 2.05). The
operator's procedure is the [close runbook](runbooks/close-period.md).

- **One open period.** A partial unique index allows one `open` and one
  `closing` period. A period only moves `open` → `closing` → `closed`, and
  nothing else about it changes (`MZ008`); periods are never deleted.
- **The switch.** Each posting takes `pg_advisory_xact_lock_shared` on the
  period lock, then reads the open period, and holds the lock until it
  commits. `begin_close` takes the lock exclusively: it waits for postings
  under way, records the period's `last_sequence`, marks it `closing` and
  opens the next one, in one transaction.
- **Into an open period only.** A trigger refuses an entry or a line whose
  period is not open when it is written, and a deferred trigger checks the
  entry again at commit (`MZ008`). Entries posted before migration 0007
  have no period.
- **Snapshots.** `finish_close` verifies the closing period in one
  `REPEATABLE READ` transaction, then writes `period_snapshots` and marks the
  period `closed`. Snapshots are append-only.

## Crash safety

Every write is one transaction (step 1.15). What a crash leaves behind
depends only on whether `COMMIT` returned:

- **Before `COMMIT`:** nothing. PostgreSQL rolls the transaction back,
  whether the process died, the connection dropped or the server was
  killed. Lines, totals, the idempotency record and any hold change are
  gone together.
- **After `COMMIT`:** everything. The commit is in the WAL on disk, and
  crash recovery replays it on the next start.
- **In between, the client may not know which.** The commit can succeed
  and the answer still be lost. The client retries with the same
  idempotency key: if the first attempt committed, it gets the original
  entry back; if not, the retry posts it once.

This was tested at four points inside every write (`FaultPoint`, with the
engine's test-only `test-hooks` feature), by killing PostgreSQL with
SIGKILL under load 20 times (`scripts/chaos.sh`), and with the simulator
(`mezgeb-sim`). See the
[1.15 evidence](evidence/1.15-failure-injection-and-crash-recovery.md).

## What one posting costs

Measured with `pg_stat_statements` during the step 1.16 baseline
(`mezgeb-bench --statements`). An entry of *n* lines on *a* accounts
runs, in one transaction:

| Statements | What |
|---|---|
| 1 | Lock the accounts: `SELECT … FOR NO KEY UPDATE`, in account-id order |
| *a* | Update each account's totals |
| 1 | Insert the entry |
| 1 | Insert the lines, all in one statement |
| 1 | Insert the idempotency record |
| — | PostgreSQL's own foreign-key checks: each line's entry and account, and the idempotency record's entry |
| *n* + 1, × 2 | At `COMMIT`: the deferred balance check (`journal_entries_balanced`, `journal_lines_balanced`) |

**The deferred balance check runs once per row**, for the entry and for
each line, and each run checks the whole entry: 3 times for a 2-line
entry, 5 times for a 4-line one. Each run is cheap (about 0.03 ms), but
the work grows with the number of lines. Checking once per entry is a
possible optimisation for step 3.01.

**On a hot account, the time is in the lock queue, not the work.** With
16 connections posting to one account, the lock `SELECT` took 40–45 ms
on average while the rest of the posting took under 2 ms.

## Error codes

Triggers raise Mezgeb's own SQLSTATE codes so callers and tests can tell
exactly which rule fired. Standard codes are used for standard constraints.

| SQLSTATE | Meaning |
|---|---|
| `MZ001` | The journal is append-only (INV-9) |
| `MZ002` | Entry unbalanced or with fewer than 2 lines (INV-1, INV-4) |
| `MZ003` | Invalid reversal (INV-14) |
| `MZ004` | Line posted to an account that is not open (INV-24) |
| `MZ005` | Forbidden account change: identity column, reopening, totals going down |
| `MZ006` | Forbidden hold change: an ended hold changed, or a change other than ending it (INV-16) |
| `MZ007` | Invalid refund, a refund tally changed other than by growing, or a reversal of a refund or of a refunded entry (INV-15) |
| `MZ008` | Posting into a period that is not open, or a forbidden period change (INV-10) |
| `23514` | A `CHECK` constraint failed |
| `23505` | A `UNIQUE` constraint failed |
| `23503` | A foreign key failed |
| `42501` | The role lacks the privilege |

## Roles

Roles are created once per PostgreSQL cluster, before migrations, by
[`db/init/01-roles.sql`](../db/init/01-roles.sql).

| Role | Used by | Privileges |
|---|---|---|
| `mezgeb_owner` | Migrations only | Owns the schema. Never used by the running service. |
| `mezgeb_app` | The ledger service | SELECT and INSERT on accounts and the journal; UPDATE on `accounts.debits_total`, `credits_total`, `version` and `status`, on `refund_tallies.refunded`, and on a period's `status` and end, only. No DELETE anywhere. |
| `mezgeb_readonly` | Reports, reconciliation, support | SELECT only |
| `mezgeb_verifier` | `mezgeb-cli verify` | SELECT on the ledger; SELECT and INSERT on the verification tables; nothing else (separation of duties, threat TM-15) |

`mezgeb_app` also has `transaction_timeout = 30s`, which verification
relies on (see [Verification](#verification)). Both are set in
[`db/init/02-verifier.sql`](../db/init/02-verifier.sql); a database created
before step 1.13 needs that file run once.

The triggers apply to every role, including the owner. Privileges are a
second, independent layer for the service.

## Running it locally

```sh
docker compose up -d     # PostgreSQL 18 on port 55432, roles created on first start
MEZGEB_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:55432/postgres \
    cargo test -p mezgeb-engine
docker compose down      # stop and delete the data
```

Each database test creates its own throwaway database, owned by
`mezgeb_owner`, runs the migrations as `mezgeb_owner`, and then acts as
`mezgeb_app`. The superuser URL is used only to create and drop those
databases. Without the variable the tests are skipped locally; on CI they
must run.

## Migrations

Migrations are SQL files in [`migrations/`](../migrations/), applied with
SQLx. Each has an `up` and a `down` file, and a test runs them up, down and
up again on an empty database. Once a migration has been applied anywhere
outside development, it is never edited: a change is a new migration.
