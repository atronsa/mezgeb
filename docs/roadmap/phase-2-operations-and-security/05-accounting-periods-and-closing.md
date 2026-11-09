# 2.05 Accounting Periods, Closing and Trial Balance

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.04 Refunds and Disputes](04-refunds-and-disputes.md) · Next: [2.06 REST API Design (Contract First)](06-rest-api-design.md)

---

**1. Overall Objective**
Add accounting periods, a close procedure, period snapshots and trial balance
reports.

**2. The Problem We Are Solving**
Without closed periods, reports change after they were issued and audits
cannot rely on them.

**3. Concepts and Research**
- Periods (daily operational, monthly accounting).
- Open → closing → closed.
- Effective date vs posting date.
- Late postings go to the current open period with a reference to the original date.
- Balance snapshots.
- How production ledgers implement a daily close.

**4. Implementation Tasks**
- `periods` and `period_snapshots` tables.
- `close_period`:
  1. Block new postings dated in it.
  2. Run verify for the period.
  3. Write snapshots.
  4. Mark closed.
- A DB trigger rejecting lines whose entry's period is closed.
- Reports: `trial_balance(as_of)` and `general_ledger(account, from, to)`.

**5. Testing Requirements**
- Posting into a closed period is rejected (engine and DB).
- Snapshot + lines after it = current balance.
- Close while postings are in flight: no posting lands in the closed period (concurrency test).
- The trial balance always balances.

**6. Deliverables**
Periods module and the two reports.

**7. Acceptance Criteria**
Snapshots verified against the full recomputation for 30 simulated days.

**8. Dependencies**
Step [1.13](../phase-1-core-engine/13-balance-verification.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The close procedure is documented as a runbook.
