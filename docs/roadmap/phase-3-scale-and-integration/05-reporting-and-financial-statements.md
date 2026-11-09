# 3.05 Reporting and Financial Statements

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.04 Partitioning, Retention and Archival](04-partitioning-and-archival.md) · Next: [3.06 Transactional Outbox and Event Integration](06-transactional-outbox.md)

---

**1. Overall Objective**
Produce the reports finance and auditors need without hurting the posting
path.

**2. The Problem We Are Solving**
Heavy reports on the primary database slow down payments, and reports that
disagree with the ledger destroy trust.

**3. Concepts and Research**
- Balance sheet and income statement from the chart of accounts.
- Account statements.
- Reporting from snapshots + deltas.
- Read replica: needed now or later? Decide on measurements.
- Replica lag implications.

**4. Implementation Tasks**
- Reports:
  - balance sheet
  - income statement
  - account statement (PDF/CSV export via CLI)
  - fee revenue by period and channel
  - escrow/offline exposure report
- Run reports from snapshots.
- A read-replica decision ADR.

**5. Testing Requirements**
- The balance sheet balances.
- Report totals equal the trial balance.
- Running reports during load: posting p99 regression stays within an agreed limit.

**6. Deliverables**
Report suite.

**7. Acceptance Criteria**
Reports reconcile to the trial balance to the santim.

**8. Dependencies**
Steps [2.05](../phase-2-operations-and-security/05-accounting-periods-and-closing.md), [3.02](../phase-3-scale-and-integration/02-indexing-and-query-optimisation.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
Reports reviewed by someone with accounting knowledge.
