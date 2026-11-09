# 3.07 Merchant Settlement Automation

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.06 Transactional Outbox and Event Integration](06-transactional-outbox.md) · Next: [3.08 Multi-Currency, Multi-Tenant and ISO 20022 Evaluation](08-multi-currency-and-multi-tenant.md)

---

**1. Overall Objective**
Automate merchant payout cycles out of the closed loop.

**2. The Problem We Are Solving**
Manual settlement is slow and error-prone. Paying a merchant twice is a real
loss.

**3. Concepts and Research**
- Settlement cycles (T+0/T+1).
- Settlement batches.
- Net settlement after fees, refunds and disputes.
- Minimum payout.
- Linking each batch to bank confirmations via reconciliation.

**4. Implementation Tasks**
- `settlement_batches` and `settlement_items` tables.
- A batch builder: lock the merchant, compute the payable amount, then place a hold via `settlement_hold.v1`.
- Payout through the withdrawal lifecycle (step 2.02).
- Unique (merchant, cycle).

**5. Testing Requirements**
- Running the batch twice for the same cycle gives one payout.
- A refund arriving during batch build: deterministic, correct result.
- A failed payout returns funds.
- Settlement reconciles with the bank statement import.

**6. Deliverables**
Settlement module.

**7. Acceptance Criteria**
A 30-day simulated settlement run reconciles exactly.

**8. Dependencies**
Steps [2.02](../phase-2-operations-and-security/02-deposits-withdrawals-and-clearing.md), [2.04](../phase-2-operations-and-security/04-refunds-and-disputes.md), [2.15](../phase-2-operations-and-security/15-reconciliation.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The settlement runbook is written.
