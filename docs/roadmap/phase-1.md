# Phase 1: Accounting Model and Core Engine (Weeks 1–16)

**Goal:** a correct, concurrent, idempotent posting engine on PostgreSQL,
proven by property, concurrency and failure tests, with no public API yet.

**Why no API yet:** an API frozen before the accounting model is understood
turns modelling mistakes into compatibility promises.

---

## Week 1: Accounting Foundations and Worked Examples

**1. Overall Objective**
Understand double-entry bookkeeping well enough to book every Atronsa money
flow on paper before any code exists.

**2. The Problem We Are Solving**
Most ledger bugs are modelling bugs: the wrong account type, a misread normal
side, or a fee booked in the wrong place. Code cannot fix a wrong model.

**3. Concepts and Research**
- Accounting equation: Assets = Liabilities + Equity (+ Revenue − Expenses).
- Account types and normal balances. For Atronsa, customer wallets are **liabilities** (money owed to users) and the bank account holding float is an **asset**.
- Debits/credits as directions, not "good/bad".
- Journal, general ledger, trial balance, suspense and clearing accounts.
- Read an introductory accounting text on journal entries and trial balances, then Atronsa's existing `ledger.rs` and its migrations.

**4. Implementation Tasks**
- Write `docs/accounting-model.md` with **at least 15 worked journal entries**: deposit via bank, withdrawal (requested, sent, failed, returned), P2P transfer, QR merchant payment with fee, card tap payment, fingerprint payment, payment request paid, full refund, partial refund, reversal, admin adjustment, merchant settlement payout, offline escrow reservation, offline receipt settlement, escrow return on expiry.
- For each one: accounts touched, side, amount, and why.
- Draft the first chart of accounts (codes and types only).

**5. Testing Requirements**
- Hand-compute a trial balance after all 15 examples. Total debits must equal total credits, and each account's balance must match its normal side.
- Ask someone with accounting knowledge to review the examples.

**6. Deliverables**
`docs/accounting-model.md`, chart of accounts v0, trial balance worksheet.

**7. Acceptance Criteria**
Every flow in the Atronsa product list has a worked entry, and the trial balance balances.

**8. Dependencies**
None.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
You can explain, without notes, why a customer deposit *credits* a liability and *debits* an asset.

---

## Week 2: Architecture Decision Records and Repository Restructure

**1. Overall Objective**
Turn `architecture.md` into binding ADRs and align the scaffold with them.

**2. The Problem We Are Solving**
Undocumented decisions get re-argued every month. Each one changed later costs a migration of real money data.

**3. Concepts and Research**
- ADR format (context, decision, alternatives, consequences).
- Modular monolith vs services.
- Multi-line journal entries vs transfer pairs (TigerBeetle's model).
- Holds as postings vs pending columns.
- REST vs gRPC.

**4. Implementation Tasks**
- Write `docs/adr/0001` to `0010`: service boundary and own DB; layering (engine/ops/client); multi-line entries; holds as postings; money as `i64` minor units; UUIDv7 IDs; READ COMMITTED + ordered row locks; idempotency in the same transaction; REST/Axum; the offline escrow model.
- Delete `proto/` and update spec §9 to REST.
- Rework the crate list: `mezgeb-core`, `mezgeb-engine` (renamed from `store-pg`), `mezgeb-ops`, `mezgeb-offline`, `mezgeb-api`, `mezgeb-sim`, `mezgeb-cli`.
- Mark the scaffold's transfer types for replacement.

**5. Testing Requirements**
- The workspace still builds and passes clippy after the restructure.
- An ADR review checklist: does each ADR list at least 2 alternatives and its consequences?

**6. Deliverables**
10 ADRs and a restructured workspace.

**7. Acceptance Criteria**
Every question in architecture.md §5 maps to an ADR.

**8. Dependencies**
Week 1.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
For any table or crate, you can point to the ADR that justifies it.

---

## Week 3: Invariant Catalogue, Threat Model v0, Regulatory Applicability

**1. Overall Objective**
Define what "correct" means precisely, who might attack the ledger, and what law actually applies.

**2. The Problem We Are Solving**
Tests without stated invariants only test the cases you thought of. Compliance assumptions made without research produce the wrong design or false claims.

**3. Concepts and Research**
- Invariants vs validations.
- STRIDE threat modelling.
- Research National Bank of Ethiopia directives for payment instrument issuers: stored value, offline limits, record retention, reporting. **Confirm applicability with a legal advisor; do not assume.**
- ISO 4217 for ETB.

**4. Implementation Tasks**
- Rewrite spec §2 as `docs/invariants.md`. For each invariant (INV-n): statement, where it is enforced (type / engine / DB / job), and how it is tested. Cover balance, immutability, idempotency, non-negative, period closure, holds, reversal uniqueness, refund cap and offline.
- Write `docs/threat-model.md` with actors (external attacker, compromised Atronsa server, malicious insider admin, malicious device owner, colluding merchant) and assets (balances, keys, audit log).
- Write `docs/regulatory.md` listing questions, sources, answers and status.

**5. Testing Requirements**
- Traceability table: every invariant has at least one planned test ID.
- Every threat has at least one planned control.

**6. Deliverables**
Three documents and the traceability matrix.

**7. Acceptance Criteria**
At least 15 invariants, at least 20 threats each with an owner and control, and the regulatory questions logged with a status.

**8. Dependencies**
Weeks 1–2.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The invariant list is reviewed and frozen at v1 (changes go through ADRs from now on).

---

## Week 4: Money, Currency and Identifier Types

**1. Overall Objective**
Make invalid money values impossible to represent in Rust.

**2. The Problem We Are Solving**
Float rounding, silent overflow, mixed currencies and swapped IDs all corrupt balances quietly.

**3. Concepts and Research**
- Minor units and the ISO 4217 exponent.
- Why not `f64`; `i64` vs `NUMERIC` (ADR-0005).
- Checked arithmetic.
- The newtype pattern.
- UUIDv7 ordering.
- JSON number precision: integers above 2^53 lose precision in JavaScript, so the API sends amounts as strings.

**4. Implementation Tasks**
In `mezgeb-core`:
- `Currency` (code and exponent; a static ISO 4217 table, ETB first).
- `Money { amount: i64, currency }` with only checked operations; adding different currencies is an error.
- `PositiveAmount`.
- Newtypes: `AccountId`, `EntryId`, `HoldId`, `ClientId`, `IdempotencyKey` (length and charset validated).
- Parsing "123.45" ↔ minor units with exact rules: too many decimals → reject, never round.
- Serde implementations that use strings.

**5. Testing Requirements**
- Property tests: parse(format(x)) == x; checked add/sub never panics and errors on overflow; cross-currency operations always error.
- Unit tests: "0.001" ETB is rejected, "-0" is rejected, max/min i64 boundaries behave.

**6. Deliverables**
`money.rs`, `currency.rs`, `ids.rs` with tests.

**7. Acceptance Criteria**
100% branch coverage on the money module, no `unwrap`, and clippy clean.

**8. Dependencies**
Week 2 (ADR-0005, 0006).

**9. Estimated Difficulty**
Low–Medium.

**10. Exit Gate**
Property tests pass for 1M cases.

---

## Week 5: Chart of Accounts and the Account Model

**1. Overall Objective**
Model accounts with type, normal side, currency, lifecycle and ownership references.

**2. The Problem We Are Solving**
A wallet with the wrong normal side, or a closed account still receiving money, breaks every report built on it.

**3. Concepts and Research**
- Account classification, normal balance by type, hierarchical codes (e.g. `2100` customer wallets, `2100-…` per wallet).
- Control accounts vs sub-ledgers.
- Account lifecycle (open → frozen → closed) and when each is allowed.
- `allow_negative` (only for specific system accounts).
- Sub-accounts for holds (`…:held`) and escrow.

**4. Implementation Tasks**
- `Account` and `AccountType`; `NormalSide` derived from the type.
- `AccountStatus` with explicit transition rules: a closed account can't reopen; it can close only at zero balance.
- `AccountTemplate` for system accounts.
- The Atronsa chart of accounts as a **data file** (`charts/atronsa.toml`), not code, so the engine stays generic.
- A balance function `balance(normal_side, debits_total, credits_total) -> i64`.

**5. Testing Requirements**
- Unit tests for every status transition, allowed and rejected.
- Property test: the balance function agrees with the hand-written normal-balance table for all account types.
- The chart file loads, codes are unique, and every system account in `accounting-model.md` exists in it.

**6. Deliverables**
Account module and chart file.

**7. Acceptance Criteria**
The 15 worked entries from Week 1 only reference accounts in the chart.

**8. Dependencies**
Weeks 1, 4.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The chart of accounts has been reviewed against the Week 1 examples.

---

## Week 6: Journal Entries and Posting Rules (Pure Core)

**1. Overall Objective**
Build the type-level guarantee that only balanced, valid entries exist.

**2. The Problem We Are Solving**
Every money bug eventually becomes "an entry that should never have been accepted".

**3. Concepts and Research**
- Journal entry vs line.
- Balancing per currency.
- Validation order and stable error codes.
- Parse-don't-validate; making illegal states unrepresentable.
- Effective date vs posted time vs event time.

**4. Implementation Tasks**
- `EntryDraft` (unvalidated) → `validate(draft, accounts_view) -> Result<BalancedEntry, PostingError>`.
- Rules: at least 2 lines; every amount > 0; one currency per line matching the account; debits = credits per currency; all accounts exist and are open; no duplicate account+side lines unless allowed; metadata size limit.
- `PostingError` enum with stable numeric codes (documented and never renumbered).
- Rewrite `mezgeb-core`, replacing the scaffold's transfer types.

**5. Testing Requirements**
- Unit test per rule, including rejection.
- Property tests: any entry `validate` accepts sums to zero per currency; random mutation of a valid entry (change one amount) is always rejected.
- The 15 worked examples are encoded as test fixtures.

**6. Deliverables**
`entry.rs`, `validate.rs`, `error.rs` and fixtures.

**7. Acceptance Criteria**
`BalancedEntry` has no public constructor other than `validate`.

**8. Dependencies**
Weeks 4–5.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
All fixtures validate, and every mutated fixture is rejected with the expected code.

---

## Week 7: In-Memory Reference Ledger (the Oracle)

**1. Overall Objective**
Build a simple, obviously-correct in-memory ledger to use as the test oracle for everything that follows.

**2. The Problem We Are Solving**
Postgres code is complex. To know it is right you need an independent model to compare it with (differential testing).

**3. Concepts and Research**
- Model-based testing.
- Differential testing.
- State-machine property testing (`proptest` state machines).
- Determinism.

**4. Implementation Tasks**
- `mezgeb-core::model::MemLedger`: create account, post entry, reverse, account totals, history.
- Single-threaded and deliberately naive.
- `check_invariants(&MemLedger)` implementing every INV that applies so far.
- A proptest state machine that generates random operation sequences and checks invariants after each step.

**5. Testing Requirements**
- 10k random sequences of 200 operations; invariants hold after every step.
- Shrinking produces minimal failing cases (verify by planting a bug).

**6. Deliverables**
`MemLedger`, invariant checker and state-machine tests.

**7. Acceptance Criteria**
A planted bug (e.g. skipping one line's total update) is caught within 1,000 cases.

**8. Dependencies**
Week 6.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The oracle and invariant checker are reviewed, and the planted-bug test is recorded.

---

## Week 8: PostgreSQL Schema v1, Constraints and Roles

**1. Overall Objective**
Design the physical schema so the database independently enforces the core invariants.

**2. The Problem We Are Solving**
Application bugs, manual SQL and future code will bypass Rust checks. The database must be the last line of defence.

**3. Concepts and Research**
- PostgreSQL data types (`BIGINT`, `UUID`, `TIMESTAMPTZ`, `TEXT` + CHECK vs ENUM).
- CHECK and FK constraints.
- Deferrable constraint triggers.
- Row-level triggers to block UPDATE/DELETE.
- Role privileges, including column-level `GRANT UPDATE`.
- Migration tooling (`sqlx migrate`).

**4. Implementation Tasks**
Migrations for:
- `currencies`
- `accounts` with:
  - CHECK on status
  - non-negative totals
  - a CHECK that forbids a negative normal-side balance unless `allow_negative`
- `journal_entries` with:
  - `UNIQUE(client_id, idempotency_key)`
  - `UNIQUE(reverses_entry_id)`
- `journal_lines` with:
  - `amount > 0`
  - side CHECK
  - a currency FK
  - `balance_after` columns
- A deferred trigger that rejects unbalanced entries.
- Triggers that reject UPDATE/DELETE on entries and lines.
- Roles:
  - `mezgeb_owner` runs migrations.
  - `mezgeb_app` gets INSERT/SELECT on journal tables, and UPDATE on the account columns `debits_total`, `credits_total`, `version` and `status` only.
  - `mezgeb_readonly` gets SELECT only.

**5. Testing Requirements**
Database tests run as `mezgeb_app`:
- An unbalanced insert fails at commit.
- UPDATE on lines fails.
- DELETE fails.
- Updating `accounts.name` fails due to privileges.
- A second reversal of the same entry fails.
- Driving a wallet negative fails via the CHECK.
- Migration up → down → up works on an empty DB.

**6. Deliverables**
Migrations, `docs/db-schema.md` and DB tests.

**7. Acceptance Criteria**
Every invariant marked "DB" in the catalogue has a passing DB test.

**8. Dependencies**
Weeks 3, 6.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
Schema review: for each constraint, name the invariant it protects.

---

## Week 9: Atomic Posting with SQLx

**1. Overall Objective**
Implement `post_entry` so one balanced entry is written atomically, with totals updated.

**2. The Problem We Are Solving**
Partial writes (lines without totals, totals without lines) are the classic ledger corruption.

**3. Concepts and Research**
- SQLx transactions and `query!` compile-time checks.
- Offline mode for CI.
- `RETURNING`.
- Batching inserts with `UNNEST`.
- Error mapping of SQLSTATE codes.
- Structured logging with `tracing`.

**4. Implementation Tasks**
`mezgeb-engine::post_entry(conn, BalancedEntry) -> PostedEntry`. In one transaction:
1. Load the accounts.
2. Re-validate.
3. Update totals.
4. Insert the entry.
5. Insert the lines with `balance_after`.
6. Commit.

Also:
- `PgStore` implements the same trait as `MemLedger`.
- Docker Compose PG 18 for tests, plus a test harness that creates a fresh database per test.

**5. Testing Requirements**
- Integration tests: all 15 fixtures post, and the totals equal the hand-computed trial balance.
- Differential test: the same random operation sequence on `MemLedger` and `PgStore` gives identical balances and history.

**6. Deliverables**
Engine posting and the test harness.

**7. Acceptance Criteria**
The differential test passes on 1,000 sequences.

**8. Dependencies**
Weeks 7–8.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
Killing the test process mid-run never leaves an entry without lines (verified by running the integrity query after a forced abort).

---

## Week 10: Concurrency Control and Concurrent Spending

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
Week 9.

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
You can explain from the Postgres docs why your scheme prevents lost updates.

---

## Week 11: Idempotency and Exactly-Once Effect

**1. Overall Objective**
Make retries safe: the same request has the same effect once.

**2. The Problem We Are Solving**
Networks drop responses. Clients retry. Without idempotency, users get charged twice.

**3. Concepts and Research**
- Idempotency keys (the IETF draft on the `Idempotency-Key` header).
- At-least-once delivery combined with idempotent receivers gives an exactly-once effect.
- Request fingerprinting.
- Key scope and retention.
- Concurrent duplicates.

**4. Implementation Tasks**
- An idempotency record in the same transaction as the entry (`client_id`, key, request hash, entry id, response snapshot, created_at).
- Canonical request serialisation for the hash.
- Concurrent same-key requests: the second waits on the unique index then returns the stored result.
- Same key with a different hash returns an `IDEMPOTENCY_KEY_REUSED` error.
- `lookup_by_idempotency_key`.
- A retention policy (the key lives at least as long as the client's retry window; document it).

**5. Testing Requirements**
- The same request 10 times sequentially gives 1 entry.
- The same request 50 times concurrently gives 1 entry and 50 identical responses.
- The same key with a different amount gives an error and no entry.
- Failure injection: abort after commit but before the response (the test hook drops the connection), then retry: the original result returns and no new entry exists.

**6. Deliverables**
Idempotency module and tests.

**7. Acceptance Criteria**
None of the tests can produce a duplicate entry.

**8. Dependencies**
Weeks 9–10.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
A crash-after-commit retry is demonstrated in a recorded test run.

---

## Week 12: Reversals and Adjustments

**1. Overall Objective**
Correct mistakes without ever editing history.

**2. The Problem We Are Solving**
"Just fix the row" destroys auditability. Fintech corrections must be visible and explainable.

**3. Concepts and Research**
- Reversal vs refund vs adjustment.
- Reversing entries on a later effective date.
- Reason codes.
- Maker-checker (four-eyes) controls.
- What happens if reversing the entry would make an account negative.

**4. Implementation Tasks**
- `reverse_entry(entry_id, reason, actor)`: mirrored lines, `reverses_entry_id`, the same idempotency semantics, and a rule against reversing a reversal.
- A rule for reversals that would breach the non-negative constraint: reject and require an adjustment (documented in an ADR).
- `adjustment` entries require `reason_code`, `requested_by` and `approved_by`, and the two must differ (enforced in the engine and by a DB CHECK).
- A table of reason codes.

**5. Testing Requirements**
- Reverse → balances return exactly to their prior values.
- A second reversal is rejected (engine and DB).
- Reversing a reversal is rejected.
- A reversal that drives an account negative is rejected.
- An adjustment with the same maker and checker is rejected.
- Property test: post then reverse leaves all balances unchanged.

**6. Deliverables**
Reversal and adjustment features with tests.

**7. Acceptance Criteria**
The original rows' bytes are unchanged after reversal (verified by a hash of the row before and after).

**8. Dependencies**
Weeks 9–11.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The adjustment control is reviewed against the threat model's insider-admin entries.

---

## Week 13: Balance Verification and Integrity Checking

**1. Overall Objective**
Continuously prove that stored balances equal the ledger.

**2. The Problem We Are Solving**
Trust in a ledger comes from **verification**, not from the fact that a balance is stored.

**3. Concepts and Research**
- Trial balance.
- Running-balance chains (`balance_after`).
- Incremental vs full verification.
- Checkpoints.
- Costs of verification queries on large tables.

**4. Implementation Tasks**
- `mezgeb-cli verify` with these checks:
  1. Each entry balances per currency.
  2. Global debits = credits per currency.
  3. Account totals = SUM(lines).
  4. The `balance_after` chain is continuous per account.
  5. No orphan entries or lines.
- A `verification_runs` table (scope, from/to checkpoint, result, duration).
- Incremental mode: only lines after the last verified checkpoint.

**5. Testing Requirements**
- Corruption tests: as superuser, deliberately tamper with one line's amount, one account total, or delete a line (bypassing triggers). Each must be detected with the exact account/entry reported.
- Run verify on 1M lines and record the time.

**6. Deliverables**
Verifier CLI, the `verification_runs` table and the corruption test suite.

**7. Acceptance Criteria**
100% of the planted corruptions are detected, and an incremental run on 1M lines finishes in under 1 minute on dev hardware (record the actual figure).

**8. Dependencies**
Weeks 8–12.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
Verify runs clean after every test suite in CI.

---

## Week 14: Holds and Two-Phase Operations

**1. Overall Objective**
Reserve funds now and capture or release them later. Card pre-auth, pending withdrawals and offline escrow all build on this.

**2. The Problem We Are Solving**
Funds promised but not yet moved must not be spendable twice, and must never get stuck forever.

**3. Concepts and Research**
- Authorisation vs capture.
- Available vs ledger balance.
- Expiry.
- Partial capture.
- Holds as postings to `…:held` sub-accounts (ADR-0004).
- Idempotent background workers.
- `SKIP LOCKED` job claiming.

**4. Implementation Tasks**
- `holds` table (id, account, held account, amount, status `active|captured|released|expired`, expires_at, entry ids).
- `place_hold` (an entry from wallet to held).
- `capture_hold` (full or partial; an entry from held to destination, with the remainder released).
- `release_hold`.
- An expiry worker that claims due holds with `FOR UPDATE SKIP LOCKED` and releases them idempotently.
- Status transitions enforced in the engine and the DB.

**5. Testing Requirements**
- Hold, then spend: available is reduced.
- Capturing twice: the second is rejected.
- Capture racing expiry concurrently: exactly one wins.
- Partial capture plus release equals the original.
- The worker crashes mid-batch and is restarted: no hold is released twice.
- Property test: the held account balance always equals SUM(active holds).

**6. Deliverables**
Holds module, expiry worker and tests.

**7. Acceptance Criteria**
The invariant "held balance = sum of active holds" holds in the oracle and in PG under concurrency.

**8. Dependencies**
Weeks 10–13.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The race test between capture and expiry passes 50 consecutive runs.

---

## Week 15: Failure Injection and Crash Recovery for the Core

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
Weeks 9–14.

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
Recorded chaos run results are committed to `docs/evidence/`.

---

## Week 16: Phase 1 Consolidation, Baseline Benchmark and Exit Review

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
Weeks 1–15.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate (Phase 1 Gate)**
- Balanced, immutable, idempotent, concurrent-safe posting.
- Holds and reversals.
- A verifier that catches all planted corruption.
- Chaos runs clean.

If any of these fails, Phase 2 does not start.
