# 1.01 Accounting Foundations and Worked Examples

[Phase 1: Accounting Model and Core Engine](README.md) · Next: [1.02 Architecture Decision Records and Repository Restructure](02-architecture-decisions.md)

---

**1. Overall Objective**
Understand double-entry bookkeeping well enough to book every common
payment flow on paper before any code exists.

**2. The Problem We Are Solving**
Most ledger bugs are modelling bugs: the wrong account type, a misread normal
side, or a fee booked in the wrong place. Code cannot fix a wrong model.

**3. Concepts and Research**
- Accounting equation: Assets = Liabilities + Equity (+ Revenue − Expenses).
- Account types and normal balances. For a wallet provider, customer wallets are **liabilities** (money owed to users) and the bank account holding float is an **asset**.
- Debits/credits as directions, not "good/bad".
- Journal, general ledger, trial balance, suspense and clearing accounts.
- Read an introductory accounting text on journal entries and trial balances, then how established open-source ledgers model accounts and entries.

**4. Implementation Tasks**
- Fill in `docs/accounting/` with **at least 15 worked journal entries**: deposit via bank, withdrawal (requested, sent, failed, returned), P2P transfer, QR merchant payment with fee, card tap payment, fingerprint payment, payment request paid, full refund, partial refund, reversal, admin adjustment, merchant settlement payout, offline escrow reservation, offline receipt settlement, escrow return on expiry.
- For each one: accounts touched, side, amount, and why.
- Draft the first chart of accounts (codes and types only).

**5. Testing Requirements**
- Hand-compute a trial balance after all 15 examples. Total debits must equal total credits, and each account's balance must match its normal side.
- Ask someone with accounting knowledge to review the examples.

**6. Deliverables**
`docs/accounting/`, chart of accounts v0, trial balance worksheet.

**7. Acceptance Criteria**
Every supported money flow has a worked entry, and the trial balance balances.

**8. Dependencies**
None.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
You can explain, without notes, why a customer deposit *credits* a liability and *debits* an asset.
