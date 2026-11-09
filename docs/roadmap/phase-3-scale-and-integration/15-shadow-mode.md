# 3.15 Shadow Mode Implementation

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.14 Historical Replay and Migration Verification](14-historical-replay.md) · Next: [3.16 Shadow Operation, Discrepancy Resolution and Phase 3 Exit](16-shadow-operation-and-phase-3-exit.md)

---

**1. Overall Objective**
Run the new ledger alongside the old one on live traffic, with no user
impact.

**2. The Problem We Are Solving**
Only real traffic reveals the cases that weren't anticipated.

**3. Concepts and Research**
- Shadow writes: the old ledger stays authoritative, the new one is asynchronous and must never block or fail the user path.
- Comparison windows.
- Discrepancy classification.

**4. Implementation Tasks**
- A client-side feature flag: after a successful old-ledger posting, enqueue an intent and post to the new ledger via the worker.
- A nightly comparison job: balances, entry counts and fee totals per account.
- A discrepancy dashboard.

**5. Testing Requirements**
- Integration tests: the new ledger being down has zero effect on the client's user path.
- Backlog drains after recovery.
- Comparison detects planted divergence.

**6. Deliverables**
Shadow pipeline and comparison job.

**7. Acceptance Criteria**
Shadow mode is enabled in production.

**8. Dependencies**
Step [3.14](../phase-3-scale-and-integration/14-historical-replay.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The first 7 days of shadow mode have every discrepancy triaged.
