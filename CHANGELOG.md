# Changelog

Notable changes to Mezgeb, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Before 1.0, a
release may change anything; see [SECURITY.md](SECURITY.md) for what is
supported.

## [Unreleased]

### Added

- **Posting templates** (`mezgeb-ops`, step 2.01): a `Template` trait that
  books a typed request against named roles, a `TemplateRegistry` keyed by
  `name.vN`, stable template errors 2001–2013, and the first template,
  `p2p_transfer.v1` (worked entry E06). Each draft records its template in
  the `template` metadata key. The engine is unchanged.
- **Deposits, withdrawals, clearing and suspense** (step 2.02): seven
  templates (`deposit_confirmed.v1`, `deposit_unmatched.v1`,
  `suspense_resolve.v1`, `withdrawal_request.v1`, `withdrawal_sent.v1`,
  `withdrawal_confirmed.v1`, `withdrawal_failed.v1`); template errors 2014
  and 2015; and an ageing query, `mezgeb_engine::ageing`, for open clearing
  and suspense items. The chart gains `9030 Clearing: withdrawals in
  transit`, and `9010 Suspense` becomes one sub-account per item. The
  clearing and suspense policy is in `docs/accounting/clearing-and-suspense.md`.

- **Merchant payments and payment requests with fees** (step 2.03):
  `merchant_payment.v1` for every merchant channel (`qr`, `card_tap`,
  `phone_tap`, `fingerprint`) and `payment_request_settle.v1`, with
  merchant-paid and payer-paid fees in the payment's own entry; template
  errors 2016–2020. ADR-0014 records the channel and fee decisions.
- **Optional amounts, optional roles and checked metadata** for templates
  (step 2.03): `Template::OPTIONAL_AMOUNTS`, `Role::optional` and
  `Template::METADATA`.

### Changed

- **Templates can read their accounts' balances** (step 2.02):
  `Template::book` takes the bound `Accounts`, by role.
- **Worked entries** (step 2.02): a withdrawal now passes through clearing.
  E03 is booked as sent and then confirmed, and E04 as sent and then rejected
  by the bank. New entries E19 and E20 book an unidentified deposit and its
  resolution.

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
