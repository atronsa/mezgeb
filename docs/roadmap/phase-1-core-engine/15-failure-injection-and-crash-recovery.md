# 1.15 Failure Injection and Crash Recovery for the Core

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.14 Holds and Two-Phase Operations](14-holds-and-two-phase-operations.md) · Next: [1.16 Phase 1 Consolidation, Baseline Benchmark and Exit Review](16-phase-1-exit-review.md)

---

**1. Overall Objective**
Prove the engine stays correct when things break mid-operation.

**2. The Problem We Are Solving**
Correctness on the happy path is not correctness. Crashes, dropped connections and restarts happen in production every week.

**3. Concepts and Research**
- Postgres WAL and crash recovery.
- What `COMMIT` guarantees.
- Client-visible ambiguity (the commit succeeded but the response was lost).
- Fault injection points.
- Deterministic simulation.

**4. Implementation Tasks**
- Fault hooks in the engine, behind a test-only feature flag: fail before lock, after lines, before commit, after commit.
- A chaos script that kills Postgres with `docker kill` during a load run, restarts it and runs verify.
- `mezgeb-sim` v1: random workload + faults against `MemLedger` and `PgStore`, comparing them.

**5. Testing Requirements**
For each fault point: retry with the same idempotency key gives exactly one entry, and verify is clean.

Postgres killed 20 times under load:
- After each restart, verify is clean.
- Every request the client saw acknowledged exists.
- Every unacknowledged request either exists exactly once or doesn't exist at all.

**6. Deliverables**
Fault hooks, chaos script and simulator v1.

**7. Acceptance Criteria**
Zero invariant violations across 20 kill cycles and 1M simulated operations.

**8. Dependencies**
Steps [1.09](../phase-1-core-engine/09-atomic-posting.md)–[1.14](../phase-1-core-engine/14-holds-and-two-phase-operations.md).

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
Recorded chaos run results are committed to `docs/evidence/`.
