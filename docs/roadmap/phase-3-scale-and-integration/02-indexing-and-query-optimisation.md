# 3.02 Indexing and Query Optimisation

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.01 Performance Methodology and Realistic Workload](01-performance-methodology.md) · Next: [3.03 Hot Accounts and Contention](03-hot-accounts-and-contention.md)

---

**1. Overall Objective**
Make history, statement, balance-at-time and report queries fast at
100M-line scale.

**2. The Problem We Are Solving**
Ledger tables only grow, so queries that are fine at 10k rows collapse at
100M.

**3. Concepts and Research**
- B-tree vs BRIN.
- Composite and covering indexes (`INCLUDE`).
- Index-only scans.
- Write amplification from each extra index.
- Autovacuum on insert-only tables.
- `EXPLAIN (ANALYZE, BUFFERS)`.

**4. Implementation Tasks**
- Generate a 100M-line dataset.
- Review every query's plan.
- Add or remove indexes, each with a measured justification.
- Tune autovacuum for append-heavy tables.
- Keyset pagination everywhere.

**5. Testing Requirements**
- Every API read query at 100M lines meets its latency target.
- Posting throughput drop from each new index is measured and recorded.

**6. Deliverables**
An index ADR with measurements and the tuned config.

**7. Acceptance Criteria**
No sequential scan on any hot-path query.

**8. Dependencies**
Step [3.01](../phase-3-scale-and-integration/01-performance-methodology.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The plan review is recorded for every query.
