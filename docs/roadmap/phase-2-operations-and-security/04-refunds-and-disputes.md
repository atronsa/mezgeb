# 2.04 Refunds and Disputes

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.03 Transfers, Merchant Payments and Fees](03-transfers-payments-and-fees.md) · Next: [2.05 Accounting Periods, Closing and Trial Balance](05-accounting-periods-and-closing.md)

---

**1. Overall Objective**
Support full and partial refunds tied to the original payment, and a dispute
flow for the closed loop.

**2. The Problem We Are Solving**
Refunding more than was paid is a direct theft path. So is refunding twice.

**3. Concepts and Research**
- Refund vs reversal (ADR).
- Cumulative refund cap.
- Fee policy on refund: whether the fee is returned (business decision, configured per template).
- Dispute lifecycle: opened → merchant funds held → resolved for payer or for merchant.

**4. Implementation Tasks**
- `refund.v1` references `original_entry_id`. The engine locks the original's refund tally row and enforces `sum(refunds) ≤ original net amount`.
- `refund_tallies` table, updated in the same transaction.
- `dispute_open.v1` (a hold on the merchant), `dispute_resolve_payer.v1`, `dispute_resolve_merchant.v1`.

**5. Testing Requirements**
- Partial refunds up to the cap succeed; one more santim is rejected.
- 50 concurrent partial refunds on one payment: the total never exceeds the cap.
- Refund after reversal: rejected.
- Dispute when the merchant's balance is insufficient: defined behaviour (reject, or hold what is available plus suspense; document it).

**6. Deliverables**
Refund and dispute templates and the tally table.

**7. Acceptance Criteria**
The over-refund property test passes 10k runs.

**8. Dependencies**
Steps [1.12](../phase-1-core-engine/12-reversals-and-adjustments.md), [1.14](../phase-1-core-engine/14-holds-and-two-phase-operations.md), [2.03](../phase-2-operations-and-security/03-transfers-payments-and-fees.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The dispute policy is written down and reviewed.
