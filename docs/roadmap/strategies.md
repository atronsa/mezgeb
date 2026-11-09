# Cross-Cutting Strategies

## 1. Testing Strategy

The goal is **evidence**: every invariant (`docs/invariants.md`) maps to the
tests that prove it, and each proof is re-run on every change.

| Layer | Tool | What it proves | When it runs |
|---|---|---|---|
| Unit | `cargo test` | Individual rules and errors | Every commit |
| Property-based | `proptest` | Rules hold for all inputs (money, validation, templates) | Every commit (short), nightly (long) |
| Model / differential | `MemLedger` vs `PgStore` | PG implementation matches a simple oracle | Every commit (short), nightly (long) |
| Database | Real PG 18 in Docker | Constraints, triggers and privileges enforce invariants independently of Rust | Every commit |
| Concurrency | Parallel tokio tasks | No double spend, no lost update, no deadlock, no over-refund | Every commit (50× loop nightly) |
| Failure injection | Fault hooks, `docker kill` | Crash at any point leaves a correct, recoverable state | Nightly |
| Deterministic simulation | `mezgeb-sim` | Millions of interleavings incl. offline adversaries | Nightly, seed printed |
| Contract | OpenAPI + Postman | API matches the spec | Every commit |
| Security | Authz matrix, fuzzing, abuse cases | Default deny, no business-logic abuse | Every commit / weekly |
| Migration | Up/down on empty DB + prod-sized copy | Migrations are safe and verify stays clean | Every migration PR |
| Performance | `mezgeb-bench` | Targets met, no regression > agreed % | Weekly + before release |
| Recovery | Restore drills | Backups restore correctly within RTO | Monthly |
| Regression | One test per fixed bug | Bugs stay fixed | Every commit |

Rules:
- **Testing only successful transactions doesn't count.** Every operation has
  tests for rejection, concurrency and failure.
- A flaky test is a failing test.
- `mezgeb-cli verify` runs after every integration suite. A dirty verify fails CI.
- Planted-bug checks prove each test layer can actually catch bugs.

## 2. Security Engineering Strategy

- **Boundaries:** the ledger has its own DB and credentials. Clients can only
  act through the authenticated API (architecture §3).
- **Default deny:** role × operation × template permission matrix with
  generated tests (W24).
- **Separation of duties:** maker-checker for adjustments, period reopen and
  device unfreeze. No single admin can move money manually.
- **Database:** app role has INSERT/SELECT on journal tables and column-limited
  UPDATE on accounts. No superuser, no DDL, triggers it can't disable (W8, W44).
- **Tamper evidence:** hash chain, signed daily checkpoints published outside
  the DB, and an audit event for every mutation (W25).
- **Offline:** hardware-rooted device keys, attestation, tier limits, escrow
  bounding loss, and conflict → freeze (W26–30).
- **Secrets:** a manager, key IDs, rotation, and HSM/KMS for the issuer key if
  the evaluation justifies it (W44).
- **Supply chain:** `cargo deny`, `cargo audit`, pinned toolchain,
  SBOM, minimal non-root images (W32, W43).
- **Verification:** threat model maintained as a living document; ASVS L2
  self-assessment; external review before go-live (W50).

## 3. Database Engineering Strategy

- **Postgres is the arbiter of money.** Every invariant that can be
  expressed in the database is expressed there too (CHECK, FK, UNIQUE,
  deferred triggers, privileges).
- **Types:** `BIGINT` minor units, `UUID` (v7), `TIMESTAMPTZ`, `TEXT` + CHECK
  for enumerations.
- **Concurrency:** READ COMMITTED, ordered `FOR NO KEY UPDATE` locks,
  `lock_timeout`, bounded retry on `40P01`/`40001` (W10).
- **Durability:** `synchronous_commit=on`, `fsync=on`, no UNLOGGED tables,
  WAL archived continuously (W42).
- **Growth:** measured indexing (W34), monthly partitioning with ahead-of-time
  creation (W36), archival without deletion inside the retention period.
- **Migrations:** forward-only in production, expand/contract, lint for unsafe
  DDL, rehearsed on a production-sized copy (W43).
- **Observability:** `pg_stat_statements`, lock wait metrics, autovacuum
  monitoring (W34, W41).

## 4. Production Deployment and Operations Strategy

- **Topology (v1):** one ledger service (≥2 instances behind a load balancer
  for rolling deploys), one Postgres primary with a streaming standby, WAL
  archive off-site. No broker, no cache.
- **Environments:** dev → staging (continuous, production-shaped data) → prod.
- **Delivery:** CI gates as in §1, signed images, health-checked rolling
  deploys, scripted rollback (W43).
- **Operations:** dashboards and actionable alerts (W41), daily verify and
  reconciliation, period close, monthly restore drill, and quarterly key
  rotation rehearsal.
- **Incidents:** severity levels, an emergency freeze procedure, and
  post-incident reviews that add regression tests (W51).

## 5. Production-Readiness Checklist

Each item needs a link to evidence. "We believe" does not count.

**Correctness**
- [ ] Every invariant has passing unit/property/DB tests (traceability matrix).
- [ ] Concurrency suite passes 50 consecutive runs.
- [ ] Chaos: 20+ Postgres kills under load, verify clean every time.
- [ ] Simulator: 14+ consecutive clean nightly runs, 10M ops each.
- [ ] Historical replay of the first client's ledger matches or every difference is explained.
- [ ] Shadow mode ≥ 30 days with zero unexplained discrepancies.

**Security**
- [ ] Authorization matrix fully tested; default deny verified.
- [ ] Maker-checker enforced for all manual money operations.
- [ ] Hash chain and checkpoint verification clean; checkpoints published.
- [ ] ASVS L2 self-assessment complete; no open high/critical findings.
- [ ] External review of offline protocol and API, or a documented reason why not.
- [ ] All secrets in a manager with tested rotation.
- [ ] App DB role verified unable to UPDATE/DELETE journal, ALTER, TRUNCATE.

**Reliability and operations**
- [ ] RPO/RTO defined; two timed restore drills passed.
- [ ] Every alert test-fired and linked to a runbook.
- [ ] Game day completed; follow-ups closed.
- [ ] Zero-downtime deploy and rollback demonstrated.
- [ ] Partition creation automated and monitored.

**Performance**
- [ ] Throughput and latency targets met with the Zipf workload at 100M+ lines.
- [ ] 24-hour soak without degradation.

**Governance**
- [ ] Regulatory applicability documented with advisor input.
- [ ] Chart of accounts and posting rules reviewed by an accountant.
- [ ] Cutover and rollback rehearsed twice; go/no-go criteria signed off.
