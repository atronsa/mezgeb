# 2.15 Reconciliation (Internal, External, Offline)

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.14 Offline Deterministic Simulation](14-offline-simulation.md) · Next: [2.16 Phase 2 Security Review and Exit](16-phase-2-security-review-and-exit.md)

---

**1. Overall Objective**
Systematically detect any disagreement between the ledger and every other
record of money.

**2. The Problem We Are Solving**
The ledger can be internally perfect and still disagree with the bank
statement, a client's cached balances or device reports.

**3. Concepts and Research**
- Reconciliation types: internal (ledger vs caches), external (settlement account vs bank statement), offline (device-reported counters vs ledger).
- Matching rules.
- Breaks, ageing and resolution.
- The rule that the ledger wins over caches.

**4. Implementation Tasks**
- `recon_runs`, `recon_items` and `recon_breaks` tables.
- Statement import (CSV first; the format comes from the partner bank; ISO 20022 camt.053 only if the bank offers it).
- Matching engine (exact, then rule-based).
- Cache reconciliation endpoint for clients: per-wallet balance comparison.
- Resolution through suspense templates only.

**5. Testing Requirements**
- Planted mismatches (missing statement line, amount mismatch, duplicate statement line, stale cache) are each detected and classified.
- Resolution posts entries; it never edits.
- Reconciling 1M items completes and its runtime is recorded.

**6. Deliverables**
Reconciliation module.

**7. Acceptance Criteria**
100% of planted breaks are detected, with 0 false matches on the test dataset.

**8. Dependencies**
Steps [2.02](../phase-2-operations-and-security/02-deposits-withdrawals-and-clearing.md), [2.05](../phase-2-operations-and-security/05-accounting-periods-and-closing.md), [2.13](../phase-2-operations-and-security/13-offline-conflicts-and-expiry.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The daily reconciliation process is documented as a runbook.
