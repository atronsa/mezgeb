# Changelog

Notable changes to Mezgeb, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Before 1.0, a
release may change anything; see [SECURITY.md](SECURITY.md) for what is
supported.

## [Unreleased]

### Added

- **Reconciliation** (ADR-0020, migration 0014, step 2.15):
  - `mezgeb-cli recon import` reads a bank statement into an account such
    as 1010: Mezgeb's CSV layout, or a bank's own export through a TOML
    mapping (`docs/reconciliation/statement-format.md`). A line already
    imported is skipped; the same line ID with other content is a
    `statement_conflict`.
  - `mezgeb-cli recon run` matches statement lines with ledger lines,
    exactly by `bank_ref` or by amount, `payment_ref` and date with one
    candidate on each side, never guessing; what is left after 2 days is a
    break (`missing_in_ledger`, `missing_in_statement`, `amount_mismatch`,
    `duplicate_statement_line`). `mezgeb-cli recon offline` finds devices
    that reported counters never received or less than they signed, and
    receipts or escrows that disagree with their entries.
  - `POST /v1/reconciliation/balances` compares a client's cached balances
    with the ledger's at the same posting sequence (`stale_cache`); the
    ledger wins.
  - Operators list breaks and runs, resolve a money break with the suspense
    entry that put 1010 right, or explain another
    (`reconciliation:read`, `reconciliation:resolve`).
  - 9040 Suspense: unexplained bank debits, `bank_debit_unmatched.v1` and
    `bank_debit_resolve.v1` (E32–E35); 5020 Bank charges is now active.
  - `mezgeb-bench --recon ITEMS` times reconciliation at scale and checks
    every planted break.
  - The daily procedure: `docs/runbooks/reconciliation.md`.
- **Offline devices and vouchers** (ADR-0019, migration 0011, step 2.11):
  - Devices register a P-256 key and get a certificate signed by the
    issuer key (`POST /v1/offline/devices`); clients revoke them.
  - Vouchers reserve escrow by E16 (`issue_voucher.v1`) and are signed
    (`POST /v1/offline/vouchers`), within the tier limit and the 5,000 ETB
    per-customer cap, one open voucher per device.
  - `GET /v1/offline/issuer-keys` publishes the issuer keys.
  - `mezgeb-cli offline` generates the issuer key, sets a client's tier
    limits, freezes and revokes devices, and checks escrow.
  - Test vectors for firmware in `test-vectors/offline/`.
  - Until attestation is verified, devices register as tier C only, which
    a client must enable.
- **Offline receipts** (migration 0012, step 2.12):
  - `POST /v1/offline/receipts` uploads up to 100 signed receipts from the
    payer or the payee; each settles exactly once (E17,
    `offline_receipt_settle.v1`), whatever the order or the repeats.
  - Receipts wait for missing earlier counters, and settle when they come.
  - Conflicts and over-spends freeze the device and its voucher, keep the
    evidence, and raise an `offline.freeze` audit event.
  - `GET /v1/offline/vouchers/{id}/receipts`; `mezgeb-cli offline fee`
    for a client's merchant fee on offline payments.
- **The rest of a voucher's life** (migration 0013, step 2.13):
  - The service ticks every minute: receipts waiting 72 hours settle past
    their gap; vouchers expire, return their escrow after 7 days of grace
    (E18), and end after the late deadline.
  - Late receipts are paid by the payer's wallet, then 5030, with the
    shortfall recorded against the payer (E28, E29); an unpaid shortfall
    blocks new vouchers until repaid (E30, `POST
    /v1/offline/shortfalls/{id}/repay`).
  - Each freeze opens a dispute; operators resolve it `paid` or `refused`
    (`offline:resolve`), and the escrow left returns (E27).
  - `mezgeb-cli offline deadline` sets a client's late deadline.
  - The ten adversarial scenarios of step 2.10 run as integration tests.
- **Tamper evidence** (ADR-0018, migration 0010):
  - Every entry is hash-chained in its own posting transaction and signed:
    the signature is its ledger receipt, returned by the API and
    verifiable with the public key from `GET /v1/keys`.
  - Audit events for every API change and refusal, every read across
    clients, and every change to clients and credentials, hash-chained;
    `GET /v1/audit-events` for auditors (`audit:read`).
  - A signed checkpoint of both chains at each period close, listed by
    `GET /v1/checkpoints` and exported to files with
    `mezgeb-cli checkpoint export`.
  - `mezgeb-cli verify --chain [--checkpoints DIR]` and
    `mezgeb-cli key generate`.
  - The service needs `MEZGEB_SIGNING_KEY_FILE`; `close-period` needs a key
    too (or `--no-checkpoint`).
  - The chain costs throughput: postings serialise on its head until they
    commit (recorded for step 3.03).

- **Authentication and authorization** (ADR-0017, migration 0009):
  - OAuth 2.0 client credentials at `POST /oauth/token`, issuing 15-minute
    opaque tokens; secrets are kept as Argon2id hashes, tokens as SHA-256.
    Revoking a credential, or disabling its client, ends its tokens at once.
  - A permission matrix of six roles (`crates/api/permissions.toml`), with
    the templates each role may post. One credential is never both
    `operator` and `approver`.
  - Each operation's scope is checked before the request is read.
  - A client touches only its own accounts, entries and holds; anything
    else is answered as if it did not exist.
  - Rate limits per client and per credential at the token endpoint.
  - `mezgeb-cli client` and `mezgeb-cli credential`, as the new
    `mezgeb_admin` role, and the
    [clients and credentials runbook](docs/runbooks/clients-and-credentials.md).
  - A generated role × operation test over the whole contract.

- **The REST service, `mezgeb-api`** (Axum): all 42 operations of the
  v1-draft contract, calling the posting templates and the engine. Request
  IDs, a body limit, a timeout that answers `503 busy` without cancelling
  the write, a JSON access log without sensitive fields, Problem Details for
  every error, and a graceful shutdown that waits for every write. Interim
  bearer tokens from a file of hashes, with a scope check on every route,
  until step 2.08. Migration 0008: accounts record who opened them and with
  which key, and maker-checker adjustment requests wait in
  `adjustment_requests` (`MZ009`). Contract tests check every response
  against the contract; a `Dockerfile`, a compose profile, and a CI job that
  runs a Postman smoke collection against the container;
  `scripts/chaos-api.sh` runs the step 1.15 chaos test through the API.
  `mezgeb-cli migrate` applies the migrations.

- **The API contract, v1-draft** (`openapi/mezgeb.v1.yaml`): 41 paths in
  OpenAPI 3.1, with one operation per posting template, maker-checker
  adjustments, an RFC 9457 error catalogue of 68 codes with an example each
  (`docs/api/errors.md`), an OWASP API Top 10 review, and a Postman
  collection generated from it (`postman/collections/mezgeb-api-v1`). A test
  checks the contract against the template registry and the error rules.

- **Accounting periods, closing and reports** (migration 0007): every entry
  belongs to the period open when it is posted. `close_period` switches
  postings to the next period under a lock, verifies the closed one, writes
  its snapshots and closes it; nothing can be posted into a closed period
  (INV-10, ADR-0016). Reports `trial_balance` (now, at a period's end, or at
  a time) and `general_ledger`. `mezgeb-cli close-period`, and the
  [close runbook](docs/runbooks/close-period.md).

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

- **Refunds and disputes** (step 2.04): an entry can refund an earlier
  standard entry (`EntryDraft::refunds`, kind `refund`). The engine caps
  every refund per account of the original, with the original's refund
  tallies locked (migration 0006, `refund_tallies`), and the database checks
  the rule again at commit (INV-15). Templates `merchant_refund.v1`,
  `dispute_open.v1`, `dispute_resolve_payer.v1` and
  `dispute_resolve_merchant.v1`; template errors 2021 and 2022; chart
  account `2060 Disputed merchant funds`. ADR-0015 and the refund and
  dispute policy (`docs/accounting/refunds-and-disputes.md`, review pending).

### Security

- **Callers who have not authenticated cost bounded work** (ADR-0021,
  step 2.16). The security review found that a caller inventing a new
  credential ID for each token request passed the per-credential limit, and
  cost an Argon2 hash and an audit event every time. Now:
  - failed logins share one bucket for the service (20 a second, bursts
    of 100), reserved before any lookup or hash and given back on success;
    over it, the token endpoint answers `429 slow_down`;
  - an optional limit per source address (`MEZGEB_ADDRESS_FAILURE_LIMIT`,
    `MEZGEB_CLIENT_ADDRESS`), off by default, covers failed logins and
    refused bearer tokens together;
  - refusals of unauthenticated callers are audit events of their own only
    for the first per actor each minute, at most 60 a minute; the rest are
    counted in a `refusals.suppressed` event, once a minute and at shutdown;
  - a client over its own rate limit is audited under its own name.

### Changed

- The interim token file (`MEZGEB_TOKENS`, `deploy/tokens.dev.toml`) is
  replaced by clients and credentials in the database;
  `deploy/clients.dev.toml` seeds the development ones.
- A missing or malformed query string is answered with Problem Details.
- `getAdjustment` accepts `adjustments:approve` as well as
  `adjustments:request`.

- **A refunded entry can no longer be reversed, and a refund is never
  reversed** (step 2.04); corrections then go through adjustments.
- **Worked entries** (step 2.04): E21–E24 book two disputes, and the trial
  balance is recomputed after E24.
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
