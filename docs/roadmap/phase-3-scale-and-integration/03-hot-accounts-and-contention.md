# 3.03 Hot Accounts and Contention

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.02 Indexing and Query Optimisation](02-indexing-and-query-optimisation.md) · Next: [3.04 Partitioning, Retention and Archival](04-partitioning-and-archival.md)

---

**1. Overall Objective**
Remove the throughput ceiling from accounts every transaction touches (fee
revenue, large merchants, escrow, chain head), but only if step 3.01 proved it
exists.

**2. The Problem We Are Solving**
Row locks serialise all postings on one account. The fee revenue account sits
in nearly every payment.

**3. Concepts and Research**
- Contention math.
- Options:
  1. Sharded sub-accounts (`fee_revenue:0..N`), summed for reporting.
  2. Deferred application for credit-only accounts that have no balance check (append the lines now, roll up the totals asynchronously). This conflicts with `balance_after`, so assess it carefully.
  3. Batching entries in one transaction.
  4. A chain-head alternative: per-partition chains merged at checkpoint.

**Recorded by step 2.09.** The journal's hash chain (ADR-0018) adds one
lock for the whole ledger: every posting holds the chain head from its last
step to its commit, so commits no longer share a disk flush. On the
development machine, 2-line entries fell from 135 to 71 per second between
uniform accounts, and from 52 to 41 on one hot account
([evidence](../../evidence/2.09-audit-logging-and-tamper-evidence.md)).
Options for this step: several chains (by account range, each with its own
head and checkpoint), or chaining in batches.

**4. Implementation Tasks**
- Measure contention in the benchmark, including the chain head.
- Choose and implement the minimal option (ADR with numbers).
- Update the verifier and reports to understand sharded accounts.

**5. Testing Requirements**
- Benchmark before and after.
- All invariant, concurrency and simulator tests still pass.
- The trial balance is unchanged by sharding.

**6. Deliverables**
Contention fix and ADR.

**7. Acceptance Criteria**
Step 3.01 throughput target met under the Zipf workload.

**8. Dependencies**
Steps [2.09](../phase-2-operations-and-security/09-audit-logging.md), [3.01](../phase-3-scale-and-integration/01-performance-methodology.md)–[3.02](../phase-3-scale-and-integration/02-indexing-and-query-optimisation.md).

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
If no fix is needed, the ADR says so, with the evidence.
