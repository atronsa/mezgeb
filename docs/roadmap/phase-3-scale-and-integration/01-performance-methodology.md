# 3.01 Performance Methodology and Realistic Workload

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [2.16 Phase 2 Security Review and Exit](../phase-2-operations-and-security/16-phase-2-security-review-and-exit.md) · Next: [3.02 Indexing and Query Optimisation](02-indexing-and-query-optimisation.md)

---

**1. Overall Objective**
Build a benchmark that reflects a real payment platform's traffic, then set targets.

**2. The Problem We Are Solving**
Optimising without a realistic workload optimises the wrong thing.

**3. Concepts and Research**
- Workload modelling: the mix of payments, holds, offline syncs and reads.
- Skew: Zipf distributions for popular merchants.
- Coordinated omission.
- Open vs closed load models.
- Sizing from the first client's projections.

**4. Implementation Tasks**
- `mezgeb-bench` with a configurable mix and skew.
- Projected volumes for years 1–3, written down with their assumptions.
- Targets: e.g. sustained postings/sec at 10× the year-1 peak, and a p99 latency target.
- Dashboards for the benchmark runs.

**5. Testing Requirements**
- Benchmark reproducibility within 10% across 3 runs.
- A 1-hour soak with no latency drift and no growth in connections or memory.

**6. Deliverables**
Benchmark tool, targets document and baseline v2.

**7. Acceptance Criteria**
Targets are written and justified by projections.

**8. Dependencies**
Phase 2.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The bottleneck is identified with evidence (`pg_stat_statements`, lock waits).
