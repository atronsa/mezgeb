# 1.13 Balance Verification and Integrity Checking

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.12 Reversals and Adjustments](12-reversals-and-adjustments.md) · Next: [1.14 Holds and Two-Phase Operations](14-holds-and-two-phase-operations.md)

---

**1. Overall Objective**
Continuously prove that stored balances equal the ledger.

**2. The Problem We Are Solving**
Trust in a ledger comes from **verification**, not from the fact that a balance is stored.

**3. Concepts and Research**
- Trial balance.
- Running-balance chains (`balance_after`).
- Incremental vs full verification.
- Checkpoints.
- Costs of verification queries on large tables.

**4. Implementation Tasks**
- `mezgeb-cli verify` with these checks:
  1. Each entry balances per currency.
  2. Global debits = credits per currency.
  3. Account totals = SUM(lines).
  4. The `balance_after` chain is continuous per account.
  5. No orphan entries or lines.
- A `verification_runs` table (scope, from/to checkpoint, result, duration).
- Incremental mode: only lines after the last verified checkpoint.

**5. Testing Requirements**
- Corruption tests: as superuser, deliberately tamper with one line's amount, one account total, or delete a line (bypassing triggers). Each must be detected with the exact account/entry reported.
- Run verify on 1M lines and record the time.

**6. Deliverables**
Verifier CLI, the `verification_runs` table and the corruption test suite.

**7. Acceptance Criteria**
100% of the planted corruptions are detected, and an incremental run on 1M lines finishes in under 1 minute on dev hardware (record the actual figure).

**8. Dependencies**
Steps [1.08](../phase-1-core-engine/08-postgresql-schema.md)–[1.12](../phase-1-core-engine/12-reversals-and-adjustments.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
Verify runs clean after every test suite in CI.
