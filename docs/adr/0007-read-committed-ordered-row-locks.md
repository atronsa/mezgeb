# 0007. READ COMMITTED with row locks taken in account-id order

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-10 |
| Answers | [Architecture §5.4](../architecture.md#54-preventing-concurrent-spending-of-the-same-balance) |
| Implemented in | 1.10 (concurrency control), 3.03 (hot accounts) |

## Context

Two payments from the same wallet can arrive at the same moment. If both
read a balance of 100 and both spend 80, the wallet ends at −60. This is the
classic double spend, and preventing it is the ledger's main job.

PostgreSQL offers several ways to prevent it. They differ in how they
behave under contention, especially on *hot accounts* such as a busy
merchant or the fee revenue account, which appear in many entries at once.

## Decision

Postings run at **`READ COMMITTED`** and take **pessimistic row locks**:

1. `SELECT … FOR UPDATE` on every account in the entry, **in ascending
   account-id order**. Every transaction locks in the same order, so two
   postings can never wait on each other in a cycle.
2. With the locks held, check balances and limits.
3. Update the account totals and insert the lines in the same transaction.

Backstops:

- A `CHECK` constraint makes it impossible to commit a balance below zero
  on its normal side for an account that does not allow negative balances.
- Deadlock (`40P01`) and serialization failure (`40001`) are retried a
  bounded number of times, then reported as an error.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **READ COMMITTED + ordered row locks (chosen)** | Correct, predictable and simple to reason about. Contention shows up as waiting, not as failures. | Postings to one hot account are serialised. Needs a separate strategy for hot accounts (step 3.03). |
| `SERIALIZABLE` everywhere | Correct without explicit locks. | Under contention, many transactions fail and must retry, so latency spikes exactly when load is highest. |
| Optimistic concurrency (`version` check) | No locks held while reading. | Retry storms on popular accounts. More work wasted under load. |
| Single writer (one queue for all postings) | No concurrency bugs possible. | Throughput limited to one thread. A single point of failure. |

## Consequences

**Positive:**

- No double spend, and no deadlocks from lock order.
- Behaviour under load is measurable and explainable.

**Negative:**

- Throughput on a single hot account is limited by lock hold time. Step
  3.03 decides how to split hot accounts.
- Every code path that posts must lock in the same order. Tests must cover
  this (step 1.10).

**Evidence:** step 1.10 adds measurements to this ADR, and step 3.03 checks
them under load.
