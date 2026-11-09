# 2.02 Deposits, Withdrawals, Clearing and Suspense Accounts

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.01 Operations Layer and Posting Templates](01-posting-templates.md) · Next: [2.03 Transfers, Merchant Payments and Fees](03-transfers-payments-and-fees.md)

---

**1. Overall Objective**
Book money entering and leaving the closed loop, including the uncertain
period while an external party confirms.

**2. The Problem We Are Solving**
External money is asynchronous and fallible. Without clearing accounts, money
"in flight" is either invisible or double counted.

**3. Concepts and Research**
- Clearing accounts (in flight) vs suspense accounts (unidentified or unresolved).
- Float / settlement bank asset account.
- Withdrawal lifecycle: requested → hold → sent → confirmed | failed → released.
- Deposits that cannot be matched to a user.

**4. Implementation Tasks**
- Templates: `deposit_confirmed.v1`, `deposit_unmatched.v1` (to suspense), `suspense_resolve.v1`, `withdrawal_request.v1` (hold), `withdrawal_sent.v1` (held → clearing), `withdrawal_confirmed.v1` (clearing → bank asset), `withdrawal_failed.v1` (clearing → wallet).
- An ageing query for suspense and clearing balances.

**5. Testing Requirements**
- Full happy path and every failure path; balances are checked at each step.
- A failed withdrawal returns exactly the original amount.
- Confirm-after-fail and fail-after-confirm are both rejected.
- Clearing nets to zero after every completed lifecycle.

**6. Deliverables**
Seven templates and the ageing report.

**7. Acceptance Criteria**
After 10k random deposit/withdrawal lifecycles, clearing = SUM(in-flight withdrawals).

**8. Dependencies**
Steps [1.14](../phase-1-core-engine/14-holds-and-two-phase-operations.md), [2.01](../phase-2-operations-and-security/01-posting-templates.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The suspense and clearing account policies are documented in the accounting model (`docs/accounting/`).
