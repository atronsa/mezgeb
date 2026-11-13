# Changelog

Notable changes to Mezgeb, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Before 1.0, a
release may change anything; see [SECURITY.md](SECURITY.md) for what is
supported.

## [Unreleased]

## [0.1.0-engine] - 2026-11-13

Milestone M1, the end of Phase 1: a posting engine on PostgreSQL, proven
correct by property, concurrency, failure and simulation tests. It has no
API yet and is **not production ready**. The evidence for every step is in
[docs/evidence/](docs/evidence/README.md).

### Added

- **Core types** (`mezgeb-core`): money as checked `i64` minor units with
  ISO 4217 currencies; UUIDv7 identifiers; a chart of accounts loaded from
  TOML; multi-line journal entries that balance per currency, validated
  into a `BalancedEntry`.
- **Reference ledger** (`MemLedger`): an in-memory ledger that gives the
  right answer to every operation, with an invariant checker.
- **PostgreSQL schema** (migrations 0001–0005): constraints, deferred
  balance triggers and role privileges that enforce the core invariants
  even if application code is wrong; append-only history.
- **Posting engine** (`mezgeb-engine`):
  - atomic posting with stored running totals
  - row locks in account-id order, so there are no deadlocks
  - idempotency keys applied once per client
  - reversals and maker-checker adjustments
  - holds (place, capture, release, expire) with an expiry worker
- **Verification** (`mezgeb-cli verify`): proves stored balances against
  the journal, fully or incrementally, and finds every planted corruption.
- **Testing tools**:
  - fault hooks at four points in every write (`test-hooks` feature)
  - a chaos script that kills PostgreSQL under load (`scripts/chaos.sh`)
  - a deterministic simulator (`mezgeb-sim`)
  - a baseline benchmark (`mezgeb-bench`)
  - coverage reports in CI

### Fixed

- A posting could wait longer than its `lock_timeout` for a busy account,
  because PostgreSQL applies `lock_timeout` to each lock wait separately.
  Each write's transaction now also sets `statement_timeout`, so the
  timeout bounds the whole wait (#42).

[Unreleased]: https://github.com/atronsa/mezgeb/compare/v0.1.0-engine...HEAD
[0.1.0-engine]: https://github.com/atronsa/mezgeb/releases/tag/v0.1.0-engine
