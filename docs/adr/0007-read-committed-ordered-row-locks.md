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

## Evidence (step 1.10)

**Implementation.** Accounts are locked with `SELECT … FOR NO KEY UPDATE`
(not `FOR UPDATE`): it blocks other postings to the same account but not
the `FOR KEY SHARE` locks that PostgreSQL takes when a journal line's foreign
key checks its account. Each transaction sets `lock_timeout` (5 s); a
deadlock (`40P01`) or serialization failure (`40001`) re-runs the whole
transaction, at most 3 attempts with jittered backoff.

**Measurements.** The same workloads under each scheme, 20 connections, 8
worker threads, PostgreSQL 18.6 in Docker Desktop on Windows 11
(`cargo run --release --example contention`). "Gave up" means the operation
still conflicted after 3 attempts.

| Workload | Scheme | Posted | Insufficient funds | Gave up | Other errors | Retry rate | Deadlock retries |
|---|---|---:|---:|---:|---:|---:|---:|
| Hot wallet: 100 × spend 80 from 100 | **RC + locks** | 1 | 99 | 0 | 0 | 0.0% | 0 |
| | SERIALIZABLE + locks | 1 | 99 | 0 | 0 | 1.0% | 0 |
| | SERIALIZABLE, no locks | 1 | 99 | 0 | 0 | 1.0% | 0 |
| | RC, no locks | 1 | 98 | 0 | **1** | 0.0% | 0 |
| 1,000 random transfers, 10 wallets | **RC + locks** | 1,000 | 0 | 0 | 0 | 0.0% | 0 |
| | SERIALIZABLE + locks | 217 | 0 | **783** | 0 | 176.8% | 0 |
| | SERIALIZABLE, no locks | 254 | 0 | **746** | 0 | 173.1% | 0 |
| | RC, no locks | 1,000 | 0 | 0 | 0 | 0.0% | 0 |
| 1,000 payments into one merchant | **RC + locks** | 1,000 | 0 | 0 | 0 | 0.0% | 0 |
| | SERIALIZABLE + locks | 144 | 0 | **856** | 0 | 180.2% | 0 |
| | SERIALIZABLE, no locks | 144 | 0 | **856** | 0 | 182.3% | 0 |
| | RC, no locks | 1,000 | 0 | 0 | 0 | 0.0% | 0 |

**What the measurements show:**

- **The chosen scheme never retried and never deadlocked**, on any
  workload, and refused exactly the 99 overdrafts.
- **SERIALIZABLE is correct but unusable under contention.** 73–86% of
  operations still failed after three attempts, even with the row locks.
  Serializable snapshot isolation also tracks the *reads* each posting makes
  (for example, checking that an entry ID is not taken), and concurrent
  inserts into the same tables conflict with them. This confirms the
  decision.
- **Without the row locks, READ COMMITTED checks stale balances.** In the hot
  wallet run, one overdraft passed the balance check in Rust and was stopped
  only by the database's `accounts_not_negative` CHECK. The locks are what
  make the Rust check authoritative; for rules with no database backstop
  (refund caps, escrow caps), there would be nothing behind it.
- **Absolute throughput here is not meaningful** (Docker Desktop's network
  on Windows dominates each round trip). Throughput is measured properly in
  step 3.01.
