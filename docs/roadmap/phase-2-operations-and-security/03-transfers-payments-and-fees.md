# 2.03 Transfers, Merchant Payments and Fees

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.02 Deposits, Withdrawals, Clearing and Suspense Accounts](02-deposits-withdrawals-and-clearing.md) · Next: [2.04 Refunds and Disputes](04-refunds-and-disputes.md)

---

**1. Overall Objective**
Book P2P transfers and merchant payments from every channel (QR, card tap,
phone tap, fingerprint), with fees in the same entry.

**2. The Problem We Are Solving**
A payment and its fee must succeed or fail together. A separate fee entry can
fail on its own and leave the books wrong.

**3. Concepts and Research**
- Gross vs net settlement to the merchant.
- Payer-paid vs merchant-paid fees.
- Fee revenue as revenue.
- Channel recorded as metadata, not as separate accounts (decide in an ADR).
- Fee calculation stays with the client; the ledger never rounds.

**4. Implementation Tasks**
- Templates: `merchant_payment.v1` (payer → merchant, optional fee lines to fee revenue) and `payment_request_settle.v1`.
- A `channel` metadata field with an enumerated set.
- Validate that fee ≤ amount and that all lines share one currency.

**5. Testing Requirements**
- Payer-paid and merchant-paid fee variants.
- A zero fee produces no fee line.
- Fee > amount is rejected.
- Concurrent payments to one merchant from 100 payers: merchant total = sum of the net amounts.

**6. Deliverables**
Payment templates.

**7. Acceptance Criteria**
Fee revenue in the trial balance equals the sum of the fee lines.

**8. Dependencies**
Step [2.01](../phase-2-operations-and-security/01-posting-templates.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
Every channel in the product list books through a template.
