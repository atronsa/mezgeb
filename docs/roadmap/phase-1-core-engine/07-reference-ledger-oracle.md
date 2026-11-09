# 1.07 In-Memory Reference Ledger (the Oracle)

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.06 Journal Entries and Posting Rules (Pure Core)](06-journal-entries-and-posting-rules.md) · Next: [1.08 PostgreSQL Schema v1, Constraints and Roles](08-postgresql-schema.md)

---

**1. Overall Objective**
Build a simple, obviously-correct in-memory ledger to use as the test oracle for everything that follows.

**2. The Problem We Are Solving**
Postgres code is complex. To know it is right you need an independent model to compare it with (differential testing).

**3. Concepts and Research**
- Model-based testing.
- Differential testing.
- State-machine property testing (`proptest` state machines).
- Determinism.

**4. Implementation Tasks**
- `mezgeb-core::model::MemLedger`: create account, post entry, reverse, account totals, history.
- Single-threaded and deliberately naive.
- `check_invariants(&MemLedger)` implementing every INV that applies so far.
- A proptest state machine that generates random operation sequences and checks invariants after each step.

**5. Testing Requirements**
- 10k random sequences of 200 operations; invariants hold after every step.
- Shrinking produces minimal failing cases (verify by planting a bug).

**6. Deliverables**
`MemLedger`, invariant checker and state-machine tests.

**7. Acceptance Criteria**
A planted bug (e.g. skipping one line's total update) is caught within 1,000 cases.

**8. Dependencies**
Step [1.06](../phase-1-core-engine/06-journal-entries-and-posting-rules.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The oracle and invariant checker are reviewed, and the planted-bug test is recorded.
