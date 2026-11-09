# 1.12 Reversals and Adjustments

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.11 Idempotency and Exactly-Once Effect](11-idempotency.md) · Next: [1.13 Balance Verification and Integrity Checking](13-balance-verification.md)

---

**1. Overall Objective**
Correct mistakes without ever editing history.

**2. The Problem We Are Solving**
"Just fix the row" destroys auditability. Fintech corrections must be visible and explainable.

**3. Concepts and Research**
- Reversal vs refund vs adjustment.
- Reversing entries on a later effective date.
- Reason codes.
- Maker-checker (four-eyes) controls.
- What happens if reversing the entry would make an account negative.

**4. Implementation Tasks**
- `reverse_entry(entry_id, reason, actor)`: mirrored lines, `reverses_entry_id`, the same idempotency semantics, and a rule against reversing a reversal.
- A rule for reversals that would breach the non-negative constraint: reject and require an adjustment (documented in an ADR).
- `adjustment` entries require `reason_code`, `requested_by` and `approved_by`, and the two must differ (enforced in the engine and by a DB CHECK).
- A table of reason codes.

**5. Testing Requirements**
- Reverse → balances return exactly to their prior values.
- A second reversal is rejected (engine and DB).
- Reversing a reversal is rejected.
- A reversal that drives an account negative is rejected.
- An adjustment with the same maker and checker is rejected.
- Property test: post then reverse leaves all balances unchanged.

**6. Deliverables**
Reversal and adjustment features with tests.

**7. Acceptance Criteria**
The original rows' bytes are unchanged after reversal (verified by a hash of the row before and after).

**8. Dependencies**
Steps [1.09](../phase-1-core-engine/09-atomic-posting.md)–[1.11](../phase-1-core-engine/11-idempotency.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The adjustment control is reviewed against the threat model's insider-admin entries.
