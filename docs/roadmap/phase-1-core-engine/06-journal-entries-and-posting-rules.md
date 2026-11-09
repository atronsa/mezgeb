# 1.06 Journal Entries and Posting Rules (Pure Core)

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.05 Chart of Accounts and the Account Model](05-chart-of-accounts.md) · Next: [1.07 In-Memory Reference Ledger (the Oracle)](07-reference-ledger-oracle.md)

---

**1. Overall Objective**
Build the type-level guarantee that only balanced, valid entries exist.

**2. The Problem We Are Solving**
Every money bug eventually becomes "an entry that should never have been accepted".

**3. Concepts and Research**
- Journal entry vs line.
- Balancing per currency.
- Validation order and stable error codes.
- Parse-don't-validate; making illegal states unrepresentable.
- Effective date vs posted time vs event time.

**4. Implementation Tasks**
- `EntryDraft` (unvalidated) → `validate(draft, accounts_view) -> Result<BalancedEntry, PostingError>`.
- Rules: at least 2 lines; every amount > 0; one currency per line matching the account; debits = credits per currency; all accounts exist and are open; no duplicate account+side lines unless allowed; metadata size limit.
- `PostingError` enum with stable numeric codes (documented and never renumbered).
- Rewrite `mezgeb-core`, replacing the scaffold's transfer types.

**5. Testing Requirements**
- Unit test per rule, including rejection.
- Property tests: any entry `validate` accepts sums to zero per currency; random mutation of a valid entry (change one amount) is always rejected.
- The 15 worked examples are encoded as test fixtures.

**6. Deliverables**
`entry.rs`, `validate.rs`, `error.rs` and fixtures.

**7. Acceptance Criteria**
`BalancedEntry` has no public constructor other than `validate`.

**8. Dependencies**
Steps [1.04](../phase-1-core-engine/04-money-and-identifier-types.md)–[1.05](../phase-1-core-engine/05-chart-of-accounts.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
All fixtures validate, and every mutated fixture is rejected with the expected code.
