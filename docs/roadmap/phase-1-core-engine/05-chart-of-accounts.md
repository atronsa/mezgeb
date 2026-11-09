# 1.05 Chart of Accounts and the Account Model

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.04 Money, Currency and Identifier Types](04-money-and-identifier-types.md) · Next: [1.06 Journal Entries and Posting Rules (Pure Core)](06-journal-entries-and-posting-rules.md)

---

**1. Overall Objective**
Model accounts with type, normal side, currency, lifecycle and ownership references.

**2. The Problem We Are Solving**
A wallet with the wrong normal side, or a closed account still receiving money, breaks every report built on it.

**3. Concepts and Research**
- Account classification, normal balance by type, hierarchical codes (e.g. `2100` customer wallets, `2100-…` per wallet).
- Control accounts vs sub-ledgers.
- Account lifecycle (open → frozen → closed) and when each is allowed.
- `allow_negative` (only for specific system accounts).
- Sub-accounts for holds (`…:held`) and escrow.

**4. Implementation Tasks**
- `Account` and `AccountType`; `NormalSide` derived from the type.
- `AccountStatus` with explicit transition rules: a closed account can't reopen; it can close only at zero balance.
- `AccountTemplate` for system accounts.
- An example wallet-provider chart of accounts as a **data file** (`charts/wallet-provider.toml`), not code, so the engine stays generic.
- A balance function `balance(normal_side, debits_total, credits_total) -> i64`.

**5. Testing Requirements**
- Unit tests for every status transition, allowed and rejected.
- Property test: the balance function agrees with the hand-written normal-balance table for all account types.
- The chart file loads, codes are unique, and every system account in the accounting model (`docs/accounting/`) exists in it.

**6. Deliverables**
Account module and chart file.

**7. Acceptance Criteria**
The 15 worked entries from step 1.01 only reference accounts in the chart.

**8. Dependencies**
Steps [1.01](../phase-1-core-engine/01-accounting-foundations.md), [1.04](../phase-1-core-engine/04-money-and-identifier-types.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The chart of accounts has been reviewed against the step 1.01 examples.
