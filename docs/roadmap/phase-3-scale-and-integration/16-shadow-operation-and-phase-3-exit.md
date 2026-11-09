# 3.16 Shadow Operation, Discrepancy Resolution and Phase 3 Exit

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.15 Shadow Mode Implementation](15-shadow-mode.md) · Next: [4.01 Full-System Fault Injection and Recovery Game Day](../phase-4-finalization/01-game-day.md)

---

**1. Overall Objective**
Fix every root cause the shadow run reveals, and pass the Phase 3 gate.

**2. The Problem We Are Solving**
Each discrepancy is a bug in one of the two ledgers, and both matter.

**3. Concepts and Research**
- Root cause analysis.
- Distinguishing a model bug, a mapping bug, a timing artefact and an old-ledger bug.

**4. Implementation Tasks**
- Triage and fix each discrepancy.
- Add a regression test for each root cause.
- Rerun the benchmarks on production-like volumes.
- Tag `v0.9.0-rc-shadow`.

**5. Testing Requirements**
- Regression tests for every discrepancy found.
- The full suite and nightly simulator stay green.

**6. Deliverables**
Discrepancy log with root causes, and the release tag.

**7. Acceptance Criteria**
14 consecutive days with zero unexplained discrepancies.

**8. Dependencies**
Step [3.15](../phase-3-scale-and-integration/15-shadow-mode.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate (Phase 3 Gate)**
- Shadow mode clean for 14 days.
- Performance targets met.
- Backups proven.
- Alerts tested.

Shadow mode **continues** through steps 4.01–4.04.
