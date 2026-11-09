# 1.08 PostgreSQL Schema v1, Constraints and Roles

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.07 In-Memory Reference Ledger (the Oracle)](07-reference-ledger-oracle.md) · Next: [1.09 Atomic Posting with SQLx](09-atomic-posting.md)

---

**1. Overall Objective**
Design the physical schema so the database independently enforces the core invariants.

**2. The Problem We Are Solving**
Application bugs, manual SQL and future code will bypass Rust checks. The database must be the last line of defence.

**3. Concepts and Research**
- PostgreSQL data types (`BIGINT`, `UUID`, `TIMESTAMPTZ`, `TEXT` + CHECK vs ENUM).
- CHECK and FK constraints.
- Deferrable constraint triggers.
- Row-level triggers to block UPDATE/DELETE.
- Role privileges, including column-level `GRANT UPDATE`.
- Migration tooling (`sqlx migrate`).

**4. Implementation Tasks**
Migrations for:
- `currencies`
- `accounts` with:
  - CHECK on status
  - non-negative totals
  - a CHECK that forbids a negative normal-side balance unless `allow_negative`
- `journal_entries` with:
  - `UNIQUE(client_id, idempotency_key)`
  - `UNIQUE(reverses_entry_id)`
- `journal_lines` with:
  - `amount > 0`
  - side CHECK
  - a currency FK
  - `balance_after` columns
- A deferred trigger that rejects unbalanced entries.
- Triggers that reject UPDATE/DELETE on entries and lines.
- Roles:
  - `mezgeb_owner` runs migrations.
  - `mezgeb_app` gets INSERT/SELECT on journal tables, and UPDATE on the account columns `debits_total`, `credits_total`, `version` and `status` only.
  - `mezgeb_readonly` gets SELECT only.

**5. Testing Requirements**
Database tests run as `mezgeb_app`:
- An unbalanced insert fails at commit.
- UPDATE on lines fails.
- DELETE fails.
- Updating `accounts.name` fails due to privileges.
- A second reversal of the same entry fails.
- Driving a wallet negative fails via the CHECK.
- Migration up → down → up works on an empty DB.

**6. Deliverables**
Migrations, `docs/db-schema.md` and DB tests.

**7. Acceptance Criteria**
Every invariant marked "DB" in the catalogue has a passing DB test.

**8. Dependencies**
Steps [1.03](../phase-1-core-engine/03-invariants-threat-model-and-regulation.md), [1.06](../phase-1-core-engine/06-journal-entries-and-posting-rules.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
Schema review: for each constraint, name the invariant it protects.
