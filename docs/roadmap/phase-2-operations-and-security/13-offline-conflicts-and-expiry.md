# 2.13 Gaps, Conflicts, Expiry, Rejections and Disputes

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.12 Receipt Ingestion and Settlement](12-receipt-ingestion-and-settlement.md) · Next: [2.14 Offline Deterministic Simulation](14-offline-simulation.md)

---

**1. Overall Objective**
Handle every non-happy offline outcome deterministically.

**2. The Problem We Are Solving**
The dangerous cases are the unusual ones: clones, missing receipts, late
receipts after escrow returned, and receipts users dispute.

**3. Concepts and Research**
- Gap timeout policy.
- Conflict evidence preservation.
- Device freeze cascade (stop issuing, flag the account).
- Escrow return on expiry.
- Suspense for late receipts.
- A dispute workflow with human review.

**4. Implementation Tasks**
- `WAITING_FOR_GAP` handling: settle receipts with a contiguous chain, hold later ones, and set the gap deadline.
- On a conflict:
  1. Freeze the device.
  2. Store both receipts as evidence.
  3. Settle nothing beyond the last contiguous verified counter.
  4. Raise an alert.
- An expiry worker returns unspent escrow to the wallet.
- A late receipt after the return is booked to a suspense account plus a dispute case (`offline_late_receipt.v1`).
- `disputes` table and the operator endpoints to resolve them.

**5. Testing Requirements**
- A clone (two receipts with the same counter) → frozen device; neither conflicting receipt settles automatically.
- Gap filled later → settles in order.
- A gap never filled → behaviour at the deadline matches the spec.
- Expiry racing a receipt ingestion: exactly one outcome, and the accounting is correct.
- Late receipt → suspense, and the case is opened.

**6. Deliverables**
Conflict, gap and expiry handling; the dispute workflow.

**7. Acceptance Criteria**
INV-11 holds, and every scenario walkthrough from step 2.10 is an automated test.

**8. Dependencies**
Step [2.12](../phase-2-operations-and-security/12-receipt-ingestion-and-settlement.md).

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
All 10 adversarial scenarios pass as integration tests.
