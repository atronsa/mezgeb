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

### Invariants marked "DB" that later steps add

Their tables do not exist yet. Each step adds the constraint and its test.

| Invariant | What the database will enforce | Step |
|---|---|---|
| INV-10 | No entry in a closed accounting period | 2.05 |
| INV-12 | `recorded_time` strictly increasing under concurrency: measured in step 1.10 (no tie or inversion in 1,000 parallel postings), guaranteed by the hash chain's single commit order | 2.09 |
| INV-16 | A hold resolves at most once | 1.14 |
| INV-19, INV-20 | Voucher escrow cap; one settlement per `(device_id, counter)` | 2.12 |
| INV-25 | Maker and checker are different people | 1.12 |
| INV-26 | One signed ledger receipt per entry | 2.09 |

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
| `mezgeb_app` | The ledger service | SELECT and INSERT on accounts and the journal; UPDATE on `accounts.debits_total`, `credits_total`, `version` and `status` only. No DELETE anywhere. |
| `mezgeb_readonly` | Reports, reconciliation, support | SELECT only |

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
