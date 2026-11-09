# 1.09 Atomic Posting with SQLx

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.08 PostgreSQL Schema v1, Constraints and Roles](08-postgresql-schema.md) · Next: [1.10 Concurrency Control and Concurrent Spending](10-concurrency-control.md)

---

**1. Overall Objective**
Implement `post_entry` so one balanced entry is written atomically, with totals updated.

**2. The Problem We Are Solving**
Partial writes (lines without totals, totals without lines) are the classic ledger corruption.

**3. Concepts and Research**
- SQLx transactions and `query!` compile-time checks.
- Offline mode for CI.
- `RETURNING`.
- Batching inserts with `UNNEST`.
- Error mapping of SQLSTATE codes.
- Structured logging with `tracing`.

**4. Implementation Tasks**
`mezgeb-engine::post_entry(conn, BalancedEntry) -> PostedEntry`. In one transaction:
1. Load the accounts.
2. Re-validate.
3. Update totals.
4. Insert the entry.
5. Insert the lines with `balance_after`.
6. Commit.

Also:
- `PgStore` implements the same trait as `MemLedger`.
- Docker Compose PG 18 for tests, plus a test harness that creates a fresh database per test.

**5. Testing Requirements**
- Integration tests: all 15 fixtures post, and the totals equal the hand-computed trial balance.
- Differential test: the same random operation sequence on `MemLedger` and `PgStore` gives identical balances and history.

**6. Deliverables**
Engine posting and the test harness.

**7. Acceptance Criteria**
The differential test passes on 1,000 sequences.

**8. Dependencies**
Steps [1.07](../phase-1-core-engine/07-reference-ledger-oracle.md)–[1.08](../phase-1-core-engine/08-postgresql-schema.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
Killing the test process mid-run never leaves an entry without lines (verified by running the integrity query after a forced abort).
