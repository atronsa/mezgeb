# 1.16 Phase 1 Consolidation, Baseline Benchmark and Exit Review

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.15 Failure Injection and Crash Recovery for the Core](15-failure-injection-and-crash-recovery.md) · Next: [2.01 Operations Layer and Posting Templates](../phase-2-operations-and-security/01-posting-templates.md)

---

**1. Overall Objective**
Close gaps, measure baseline performance and pass the Phase 1 gate.

**2. The Problem We Are Solving**
Unfinished foundations compound. Phase 2 builds money operations on top of this engine.

**3. Concepts and Research**
- Benchmarking basics: warm-up, percentiles, coordinated omission.
- `EXPLAIN (ANALYZE, BUFFERS)`.
- `pg_stat_statements`.

**4. Implementation Tasks**
- Fix all open Phase 1 issues.
- A baseline benchmark: postings/sec and p50/p99 latency for 2-line and 4-line entries, uniform accounts vs one hot account.
- Update the docs (invariants traceability, db-schema, ADR evidence).
- Tag `v0.1.0-engine`.

**5. Testing Requirements**
- The full suite: unit, property, DB, concurrency, chaos.
- A coverage report.
- The benchmark repeated 3 times with variance under 10%.

**6. Deliverables**
Baseline benchmark report, release tag and Phase 1 review notes.

**7. Acceptance Criteria**
- Every Phase 1 invariant has a passing test.
- No open correctness bugs.
- Benchmark numbers recorded (no target yet; this is the baseline).

**8. Dependencies**
Steps [1.01](../phase-1-core-engine/01-accounting-foundations.md)–[1.15](../phase-1-core-engine/15-failure-injection-and-crash-recovery.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate (Phase 1 Gate)**
- Balanced, immutable, idempotent, concurrent-safe posting.
- Holds and reversals.
- A verifier that catches all planted corruption.
- Chaos runs clean.

If any of these fails, Phase 2 does not start.
