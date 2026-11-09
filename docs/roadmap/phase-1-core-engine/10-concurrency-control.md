# 1.10 Concurrency Control and Concurrent Spending

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.09 Atomic Posting with SQLx](09-atomic-posting.md) · Next: [1.11 Idempotency and Exactly-Once Effect](11-idempotency.md)

---

**1. Overall Objective**
Make concurrent postings safe, deadlock-free and correctly serialised per account.

**2. The Problem We Are Solving**
Two payments from a wallet holding 100, each for 80, arriving in the same millisecond: exactly one must succeed.

**3. Concepts and Research**
- PostgreSQL MVCC.
- READ COMMITTED vs REPEATABLE READ vs SERIALIZABLE.
- `SELECT … FOR UPDATE` vs `FOR NO KEY UPDATE`.
- Lock ordering.
- Deadlock detection (`40P01`) and serialization failures (`40001`).
- Lost updates and write skew.
- `lock_timeout` and `statement_timeout`.

**4. Implementation Tasks**
- Lock all entry accounts with `FOR NO KEY UPDATE` in ascending ID order before checking balances.
- A retry wrapper: max 3 attempts on `40P01`/`40001`, with jittered backoff, re-running the whole transaction.
- Set `lock_timeout` per transaction.
- Write ADR-0007's evidence section with the measurements.

**5. Testing Requirements**
Concurrency tests:
- 100 tasks each spend 80 from a 100 balance: exactly 1 succeeds, 99 get `INSUFFICIENT_FUNDS`, and the balance ends at 20.
- 1,000 random transfers among 10 accounts in parallel: the global sum is unchanged, there are no deadlock errors after retry, and totals equal SUM(lines).
- An A→B and B→A storm causes no deadlocks.
- Then try SERIALIZABLE on the same workload and record the retry rates.

**6. Deliverables**
Locking implementation, concurrency test suite and ADR evidence.

**7. Acceptance Criteria**
The tests pass 50 consecutive runs (flakiness is a failure).

**8. Dependencies**
Step [1.09](../phase-1-core-engine/09-atomic-posting.md).

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
You can explain from the Postgres docs why your scheme prevents lost updates.
