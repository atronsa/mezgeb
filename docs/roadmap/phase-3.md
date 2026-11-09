# Phase 3: Scale, Operations and Atronsa Integration (Weeks 33–48)

**Goal:** measured performance, production operations, and a verified
migration of Atronsa onto the new ledger running in shadow mode.

---

## Week 33: Performance Methodology and Realistic Workload

**1. Overall Objective**
Build a benchmark that reflects Atronsa's real traffic, then set targets.

**2. The Problem We Are Solving**
Optimising without a realistic workload optimises the wrong thing.

**3. Concepts and Research**
- Workload modelling: the mix of payments, holds, offline syncs and reads.
- Skew: Zipf distributions for popular merchants.
- Coordinated omission.
- Open vs closed load models.
- Sizing from Atronsa projections.

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

---

## Week 34: Indexing and Query Optimisation

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
Week 33.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The plan review is recorded for every query.

---

## Week 35: Hot Accounts and Contention

**1. Overall Objective**
Remove the throughput ceiling from accounts every transaction touches (fee
revenue, large merchants, escrow, chain head), but only if Week 33 proved it
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

**4. Implementation Tasks**
- Measure contention in the benchmark.
- Choose and implement the minimal option (ADR with numbers).
- Update the verifier and reports to understand sharded accounts.

**5. Testing Requirements**
- Benchmark before and after.
- All invariant, concurrency and simulator tests still pass.
- The trial balance is unchanged by sharding.

**6. Deliverables**
Contention fix and ADR.

**7. Acceptance Criteria**
Week 33 throughput target met under the Zipf workload.

**8. Dependencies**
Weeks 25, 33–34.

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
If no fix is needed, the ADR says so, with the evidence.

---

## Week 36: Partitioning, Retention and Archival

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
- Retention requirements (from Week 3 research).

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
Weeks 8, 34.

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
The archived partition restore procedure has been rehearsed.

---

## Week 37: Reporting and Financial Statements

**1. Overall Objective**
Produce the reports finance and auditors need without hurting the posting
path.

**2. The Problem We Are Solving**
Heavy reports on the primary database slow down payments, and reports that
disagree with the ledger destroy trust.

**3. Concepts and Research**
- Balance sheet and income statement from the chart of accounts.
- Account statements.
- Reporting from snapshots + deltas.
- Read replica: needed now or later? Decide on measurements.
- Replica lag implications.

**4. Implementation Tasks**
- Reports:
  - balance sheet
  - income statement
  - account statement (PDF/CSV export via CLI)
  - fee revenue by period and channel
  - escrow/offline exposure report
- Run reports from snapshots.
- A read-replica decision ADR.

**5. Testing Requirements**
- The balance sheet balances.
- Report totals equal the trial balance.
- Running reports during load: posting p99 regression stays within an agreed limit.

**6. Deliverables**
Report suite.

**7. Acceptance Criteria**
Reports reconcile to the trial balance to the santim.

**8. Dependencies**
Weeks 21, 34.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
Reports reviewed by someone with accounting knowledge.

---

## Week 38: Transactional Outbox and Event Integration

**1. Overall Objective**
Notify Atronsa reliably of ledger events (posted, hold expired, receipt
settled, conflict) with no message broker.

**2. The Problem We Are Solving**
"Write to the DB, then publish" loses or duplicates events when a crash falls
between the two steps.

**3. Concepts and Research**
- Transactional outbox.
- At-least-once delivery.
- Consumer idempotency.
- Ordering guarantees per account.
- Webhooks vs polling (Atronsa pulls via a cursor API, the simpler and safer option).

**4. Implementation Tasks**
- `outbox` table written in the posting transaction.
- `GET /v1/events?after=cursor` with ordered, paginated events.
- Optional signed webhooks later.
- A retention job.

**5. Testing Requirements**
- Crash between commit and delivery: the event is still delivered.
- Duplicate delivery is safe on the consumer side (test with an Atronsa stub).
- Ordering per account is preserved.

**6. Deliverables**
Outbox and events API.

**7. Acceptance Criteria**
In a 1M-event chaos run, zero events are lost.

**8. Dependencies**
Week 23.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The broker decision is recorded: still not needed, with the reason.

---

## Week 39: Merchant Settlement Automation

**1. Overall Objective**
Automate merchant payout cycles out of the closed loop.

**2. The Problem We Are Solving**
Manual settlement is slow and error-prone. Paying a merchant twice is a real
loss.

**3. Concepts and Research**
- Settlement cycles (T+0/T+1).
- Settlement batches.
- Net settlement after fees, refunds and disputes.
- Minimum payout.
- Linking each batch to bank confirmations via reconciliation.

**4. Implementation Tasks**
- `settlement_batches` and `settlement_items` tables.
- A batch builder: lock the merchant, compute the payable amount, then place a hold via `settlement_hold.v1`.
- Payout through the withdrawal lifecycle (Week 18).
- Unique (merchant, cycle).

**5. Testing Requirements**
- Running the batch twice for the same cycle gives one payout.
- A refund arriving during batch build: deterministic, correct result.
- A failed payout returns funds.
- Settlement reconciles with the bank statement import.

**6. Deliverables**
Settlement module.

**7. Acceptance Criteria**
A 30-day simulated settlement run reconciles exactly.

**8. Dependencies**
Weeks 18, 20, 31.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The settlement runbook is written.

---

## Week 40: Multi-Currency, Multi-Tenant and ISO 20022 Evaluation

**1. Overall Objective**
Decide what to build now and what to defer. Build only what the requirements
justify.

**2. The Problem We Are Solving**
Over-building adds risk. Under-designing forces a rewrite later.

**3. Concepts and Research**
- Multi-currency accounting: per-currency balancing, FX via liquidity accounts, revaluation.
- Multi-tenancy: a tenant column vs schema per tenant vs DB per tenant.
- ISO 20022 message families relevant to bank integration (pain, pacs, camt).

**4. Implementation Tasks**
- Three ADRs:
  - Expected: multi-currency is structurally supported but only ETB is enabled.
  - Expected: multi-tenancy is deferred, but `client_id` scoping already isolates clients.
  - Expected: ISO 20022 is deferred until a partner bank requires it.
- Verify the existing schema allows each later without migrating historical data. Add a guard test that rejects a second currency until it is enabled.

**5. Testing Requirements**
- A test that a USD entry is rejected while it isn't enabled.
- A test proving per-currency balancing already works when a second currency is enabled in a test database.

**6. Deliverables**
Three ADRs and the guard tests.

**7. Acceptance Criteria**
Each deferral has explicit triggers ("build when …").

**8. Dependencies**
Weeks 4, 6.

**9. Estimated Difficulty**
Low–Medium.

**10. Exit Gate**
The ADRs are reviewed. *Spare capacity: use the rest of this week to pay down slippage.*

---

## Week 41: Observability: Logs, Metrics, Dashboards, Alerts

**1. Overall Objective**
See the ledger's health and financial integrity in real time.

**2. The Problem We Are Solving**
Silent failures, such as a stalled expiry worker, rising suspense or a failed
verification, cost money when nobody notices them.

**3. Concepts and Research**
- Structured logging (`tracing`, JSON).
- The RED/USE methods.
- Prometheus metric types and cardinality limits.
- Alert design: actionable alerts only.
- Distributed tracing: **not justified** with one service. Request IDs propagated from Atronsa are enough; revisit if more services appear.

**4. Implementation Tasks**
- A `/metrics` endpoint.
- Technical metrics: latency, errors, lock waits, retries, pool saturation.
- Financial metrics:
  - postings by template
  - suspense balance
  - clearing ageing
  - escrow exposure
  - unresolved conflicts
  - verify result
  - checkpoint age
- Grafana dashboards as code.
- Alert rules: verify failure, checkpoint missing, suspense above threshold, a stalled worker, error rate.

**5. Testing Requirements**
- Fire every alert in staging by inducing its condition.
- No PII or secrets in logs (automated log scan test).

**6. Deliverables**
Metrics, dashboards and alert rules.

**7. Acceptance Criteria**
Every alert has a runbook link and was test-fired.

**8. Dependencies**
Weeks 13, 23, 29.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
A deliberately stalled worker is detected by the alerts within the agreed time.

---

## Week 42: Backups, Point-in-Time Recovery and Disaster Recovery

**1. Overall Objective**
Be able to restore the ledger to any point in time and prove the restore is
correct.

**2. The Problem We Are Solving**
An untested backup is a hope, not a backup. Money data cannot be lost.

**3. Concepts and Research**
- WAL archiving.
- Base backups.
- pgBackRest vs WAL-G (choose one).
- RPO and RTO definitions.
- Streaming replication for HA vs DR.
- Encryption of backups.
- Off-site copies.

**4. Implementation Tasks**
- Configure pgBackRest with encrypted, off-site storage.
- Define RPO/RTO in an ADR.
- A restore script that restores to a timestamp, runs `verify` and the chain verify, then compares with the checkpoint.
- A monthly restore drill job.

**5. Testing Requirements**
- Recovery tests: restore to a timestamp mid-load; all entries acknowledged before that time are present and verify is clean.
- Total loss of the primary → restore within the RTO (timed).

**6. Deliverables**
Backup configuration, restore tooling and the DR runbook.

**7. Acceptance Criteria**
Two successful timed restore drills recorded.

**8. Dependencies**
Weeks 13, 25.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
A restore drill is performed by following only the runbook.

---

## Week 43: Deployment and CI/CD

**1. Overall Objective**
Ship changes safely, repeatably and reversibly.

**2. The Problem We Are Solving**
Most production incidents are caused by deployments, especially schema
migrations on tables that can't be locked.

**3. Concepts and Research**
- Expand/contract migrations.
- Lock-safe DDL (`CREATE INDEX CONCURRENTLY`, `lock_timeout` in migrations).
- Immutable images.
- SBOM.
- Signed releases.
- Environment promotion (dev → staging → prod).

**4. Implementation Tasks**
- A multi-stage Dockerfile with a minimal image running as non-root.
- A CI pipeline:
  - fmt, clippy, unit, property, DB, concurrency, contract, `cargo deny`
  - the simulator smoke test
  - migration tests against a production-sized copy
- A migration linter that rejects unsafe DDL.
- A deploy script with health checks and rollback.
- Release notes template.

**5. Testing Requirements**
- Deploy N → N+1 → rollback to N under load: no errors and verify clean.
- An unsafe migration in a PR is blocked by CI.

**6. Deliverables**
Pipeline, images and deploy tooling.

**7. Acceptance Criteria**
A zero-downtime deploy is demonstrated under load.

**8. Dependencies**
Week 36.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
A staging environment runs continuously.

---

## Week 44: Secrets, Key Management and Operational Security

**1. Overall Objective**
Protect the keys and credentials whose compromise means direct financial
loss.

**2. The Problem We Are Solving**
A leaked voucher-signing key lets an attacker mint offline spending. Leaked
DB credentials bypass the API entirely.

**3. Concepts and Research**
- Secrets managers vs environment files.
- Key rotation without downtime (key IDs in vouchers).
- HSM/KMS evaluation for the issuer key.
- Postgres hardening: TLS, `scram-sha-256`, `pg_hba`, no superuser for the app, pgaudit evaluation.
- Network exposure.

**4. Implementation Tasks**
- Move all secrets to a manager.
- Key IDs on vouchers and checkpoints.
- Rotation procedures for the issuer key, checkpoint key, API credentials and DB passwords.
- DB hardening and a review of role grants.
- An HSM/KMS decision ADR.

**5. Testing Requirements**
- Security tests: a rotated issuer key — old vouchers stay valid until expiry and new ones use the new key.
- A revoked key is rejected.
- The app role cannot `ALTER`, `TRUNCATE` or disable triggers (attempt each).
- A secret scan of the repo and images comes back clean.

**6. Deliverables**
Key management and hardening, plus rotation runbooks.

**7. Acceptance Criteria**
Every secret has an owner, a location, a rotation procedure and a tested rotation.

**8. Dependencies**
Weeks 24, 27.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The threat model's key-compromise scenarios each have a tested response.

---

## Week 45: Atronsa Integration Design and Account Mapping

**1. Overall Objective**
Plan exactly how Atronsa moves from its `ledger.rs` to the new ledger.

**2. The Problem We Are Solving**
Migrating live money is the riskiest step of the whole project.

**3. Concepts and Research**
- Atronsa's current schema:
  - `ledger_accounts`
  - `ledger_transactions`
  - `ledger_entries`
  - `user_wallets`
  - fee and clearing accounts
  - daily close
- Strangler pattern.
- Dual-write vs replay.
- The client-side intent table and retry worker.

**4. Implementation Tasks**
- A mapping document: each Atronsa account → new account code; each Atronsa transaction type → template.
- Gaps and mismatches listed and resolved.
- Atronsa changes specified: intent table, ledger client, idempotency keys from transaction IDs, the event consumer.
- The cutover plan outline, with rollback.

**5. Testing Requirements**
- Every Atronsa transaction type in production data has a mapping (an automated scan of distinct types).

**6. Deliverables**
Mapping document and Atronsa integration spec.

**7. Acceptance Criteria**
Zero unmapped types or accounts.

**8. Dependencies**
Weeks 17–21, 38.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The mapping is reviewed against real (anonymised) production data.

---

## Week 46: Historical Replay and Migration Verification

**1. Overall Objective**
Replay Atronsa's full history into the new ledger and prove every balance
matches.

**2. The Problem We Are Solving**
If history doesn't reproduce, either the old ledger or the new model is wrong.
You must know which before going live.

**3. Concepts and Research**
- Replay ordering.
- Preserving original timestamps as effective dates.
- Handling historical data that violates new invariants (record and explain it; never fake it).

**4. Implementation Tasks**
- `mezgeb-cli import-atronsa`: reads an Atronsa DB snapshot and posts through the templates with idempotency keys derived from the original IDs.
- A comparison report per account.
- An exceptions log with an explanation for each item.

**5. Testing Requirements**
- Replay twice gives an identical result (idempotency).
- Every account balance equals Atronsa's balance, or the difference is in the exceptions list with a cause.
- The fee revenue total matches.

**6. Deliverables**
Importer, comparison report and exceptions log.

**7. Acceptance Criteria**
100% of accounts match or are explained, and the explained exceptions are signed off.

**8. Dependencies**
Week 45.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
A replay of a fresh production snapshot passes.

---

## Week 47: Shadow Mode Implementation

**1. Overall Objective**
Run the new ledger alongside the old one on live traffic, with no user
impact.

**2. The Problem We Are Solving**
Only real traffic reveals the cases that weren't anticipated.

**3. Concepts and Research**
- Shadow writes: the old ledger stays authoritative, the new one is asynchronous and must never block or fail the user path.
- Comparison windows.
- Discrepancy classification.

**4. Implementation Tasks**
- An Atronsa feature flag: after a successful old-ledger posting, enqueue an intent and post to the new ledger via the worker.
- A nightly comparison job: balances, entry counts and fee totals per account.
- A discrepancy dashboard.

**5. Testing Requirements**
- Integration tests: the new ledger being down has zero effect on the Atronsa user path.
- Backlog drains after recovery.
- Comparison detects planted divergence.

**6. Deliverables**
Shadow pipeline and comparison job.

**7. Acceptance Criteria**
Shadow mode is enabled in production.

**8. Dependencies**
Week 46.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The first 7 days of shadow mode have every discrepancy triaged.

---

## Week 48: Shadow Operation, Discrepancy Resolution and Phase 3 Exit

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
Week 47.

**9. Estimated Difficulty**
High.

**10. Exit Gate (Phase 3 Gate)**
- Shadow mode clean for 14 days.
- Performance targets met.
- Backups proven.
- Alerts tested.

Shadow mode **continues** through Weeks 49–52.
