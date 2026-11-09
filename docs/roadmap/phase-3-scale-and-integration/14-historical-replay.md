# 3.14 Historical Replay and Migration Verification

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.13 Client Integration Design and Account Mapping](13-client-integration-design.md) · Next: [3.15 Shadow Mode Implementation](15-shadow-mode.md)

---

**1. Overall Objective**
Replay the client's full ledger history into Mezgeb and prove every balance
matches.

**2. The Problem We Are Solving**
If history doesn't reproduce, either the old ledger or the new model is wrong.
You must know which before going live.

**3. Concepts and Research**
- Replay ordering.
- Preserving original timestamps as effective dates.
- Handling historical data that violates new invariants (record and explain it; never fake it).

**4. Implementation Tasks**
- `mezgeb-cli import`: reads a legacy ledger snapshot through a pluggable source adapter and posts through the templates with idempotency keys derived from the original IDs.
- A comparison report per account.
- An exceptions log with an explanation for each item.

**5. Testing Requirements**
- Replay twice gives an identical result (idempotency).
- Every account balance equals the legacy ledger's balance, or the difference is in the exceptions list with a cause.
- The fee revenue total matches.

**6. Deliverables**
Importer, comparison report and exceptions log.

**7. Acceptance Criteria**
100% of accounts match or are explained, and the explained exceptions are signed off.

**8. Dependencies**
Step [3.13](../phase-3-scale-and-integration/13-client-integration-design.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
A replay of a fresh production snapshot passes.
