# 3.04 Partitioning, Retention and Archival

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.03 Hot Accounts and Contention](03-hot-accounts-and-contention.md) · Next: [3.05 Reporting and Financial Statements](05-reporting-and-financial-statements.md)

---

**1. Overall Objective**
Keep table size manageable for years without ever losing financial history.

**2. The Problem We Are Solving**
Multi-billion-row tables slow down vacuum, backups and queries. Deleting
financial data is not an option.

**3. Concepts and Research**
- Declarative range partitioning by month.
- Partition pruning.
- Unique constraints on partitioned tables (they must include the partition key; this affects the idempotency index design).
- Detaching old partitions to cheaper storage.
- Retention requirements (from step 1.03 research).

**4. Implementation Tasks**
- Partition `journal_lines`, `journal_entries` and `audit_events` by month.
- Redesign global uniqueness (idempotency, reversal) with separate non-partitioned key tables if needed.
- A partition maintenance job that creates partitions ahead of time.
- Archival procedure: detach → verify → export with checksums → keep attached read-only, or move. Never delete within the retention period.
- A **migration test** from the unpartitioned schema on a 100M-row copy.

**5. Testing Requirements**
- Migration on the 100M copy completes, verify is clean, and the duration is recorded.
- Posting into a month with no partition never fails, because the job always creates partitions ahead.
- Uniqueness still holds across partitions.

**6. Deliverables**
Partitioned schema, maintenance job and archival runbook.

**7. Acceptance Criteria**
The migration is rehearsed twice with identical verify results.

**8. Dependencies**
Steps [1.08](../phase-1-core-engine/08-postgresql-schema.md), [3.02](../phase-3-scale-and-integration/02-indexing-and-query-optimisation.md).

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
The archived partition restore procedure has been rehearsed.
