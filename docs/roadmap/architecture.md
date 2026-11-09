# Overall Architecture

This document records the architectural position the roadmap is built on.
Every decision below is provisional until it is written up as an ADR
(`docs/adr/NNNN-*.md`) in Week 2, where the alternatives get re-examined.
Where a decision is deferred, the week that decides it is named.

---

## 1. What the ledger is, and what it is not

The ledger is the **system of record for money**. It answers four questions
with certainty: what each account holds, how it got there, who caused each
movement, and whether the books balance.

It is **not** the wallet product, the payment product, the fee policy, KYC,
cards, QR codes or the offline device. Those belong to the client application
that uses it. The ledger receives instructions to move value, validates them
against accounting rules, records them permanently and reports on them.

> A wallet balance is a *view* of ledger accounts. It is never the ledger.

## 2. Stack evaluation

| Choice | Verdict | Reason |
|---|---|---|
| Rust | Keep | Memory safety, strong types for money/IDs, no GC pauses, the team already knows it. |
| PostgreSQL 18 | Keep | ACID, mature row locking, deferred constraints, declarative partitioning, PITR. |
| SQLx | Keep | Compile-time-checked SQL; explicit transactions; no ORM hiding locking behaviour. Use offline mode (`.sqlx/`) in CI. |
| Axum + REST | Keep | The most widely understood API style, works with the Postman tooling already in use, easy to audit. **Replaces the gRPC draft in `proto/`**: gRPC brings no benefit at this scale that justifies a second toolchain. |
| Docker | Keep | Reproducible local Postgres, CI and deployment. |
| Redis / Kafka / message broker | **Reject** | Idempotency and events must commit in the *same* Postgres transaction as the posting. An outside store or broker would break that. The outbox table (Week 38) covers eventing. |
| Microservices | **Reject** | One ledger service, internally modular (see §3). |
| ORM (Diesel/SeaORM) | Reject | Locking order and SQL shape must be explicit and reviewable. |

Additions, each justified when introduced: `proptest` (W7), `tracing` (W9),
`ed25519-dalek` + `sha2` (W25–27), Prometheus/Grafana (W41), pgBackRest (W42),
`cargo-deny`/`cargo-audit` (W43).

## 3. Shape of the system: a modular monolith behind its own boundary

```
          Client application server               Ledger service (this repo)
   ┌─────────────────────────────────┐      ┌────────────────────────────────────┐
   │ users, KYC, QR, cards, fees     │      │ api      (Axum, REST, authn/authz) │
   │ payment flows, limits           │ REST │ ops      (posting templates)       │
   │ intent table + retry worker ────┼─────▶│ offline  (vouchers, receipts)      │
   │ cached balances (display only)  │◀─────┼ recon / reports / audit / jobs     │
   └─────────────────────────────────┘      │ engine   (post, hold, reverse)     │
                                            │ core     (pure domain, no I/O)     │
                                            └──────────────┬─────────────────────┘
                                                           │ own DB + own roles
                                                     PostgreSQL (ledger DB)
```

**Decision:** one deployable ledger service with its **own database and its
own credentials**, internally split into crates/modules.

| Alternative | For | Against |
|---|---|---|
| Embedded library in the client's DB | One transaction covers business state + posting; simplest consistency | The client's app credentials can touch ledger tables; a compromised client server can write money. No hard security boundary. Not reusable as an open-source product. |
| **Separate service, own DB (chosen)** | Hard privilege boundary; ledger roles can only INSERT entries; independently testable and releasable | Cross-system consistency must be engineered (idempotency + intent table, §5.10) |
| Several ledger microservices | None at this scale | Distributed transactions inside the money path |

The `core` crate stays pure (no I/O) so it can be tested exhaustively and
reused by the simulator. The engine is written against `&mut PgConnection` so
it *could* be embedded later without a rewrite.

## 4. Data model at a glance

- **Currency**: ISO 4217 alphabetic code + minor-unit exponent (ETB, 2).
- **Account**: id, code (chart of accounts), name, type
  (asset/liability/equity/revenue/expense), normal side, currency, status
  (open/frozen/closed), `allow_negative`, `external_ref` (e.g. a client wallet
  id), `debits_total`, `credits_total`, `version`.
- **Journal entry**: id (UUIDv7), client id + idempotency key (unique),
  template + version, reference, effective date, posted_at, period,
  `reverses_entry_id` (unique), metadata, `prev_hash`, `hash`.
- **Journal line**: entry id, account id, side (debit/credit), amount
  (`BIGINT > 0`, minor units), currency, account totals *after* this line.
- **Hold**: a real posting from an account into its `…:held` sub-account,
  plus a hold record (status, expiry, capture/release entries).
- Supporting tables: idempotency records, accounting periods + snapshots,
  audit events, outbox, offline devices/vouchers/receipts, reconciliation runs
  and breaks.

**Change from the scaffold:** the scaffold's TigerBeetle-style
"one debit, one credit per transfer" model is replaced by **multi-line
journal entries**. One merchant payment with a fee is a single
balanced entry (debit payer 100, credit merchant 98, credit fee revenue 2)
instead of a linked chain. That fits accounting practice and trial balances
better. Week 2 records this as an ADR and
Week 4–6 rewrites `mezgeb-core` accordingly.

## 5. The ten architectural questions

### 5.1 Modular monolith initially?
Yes, see §3. The service is one process. Modules communicate through Rust
interfaces, not the network. Splitting is reconsidered only if a measured
bottleneck demands it (not expected within this roadmap).

### 5.2 Separating the engine from wallet and payment logic
Three layers, each depending only on the one below:

1. **Engine** knows accounts, entries, lines, holds, reversals, periods.
   It enforces accounting rules and nothing else.
2. **Operations (posting templates)** are named, versioned functions such as
   `p2p_transfer.v1` or `merchant_payment.v1` that turn a typed request into a
   balanced entry. They encode *how* a business event is booked, never *whether*
   it is allowed.
3. **Client application** decides whether it is allowed (KYC, limits, fee amounts,
   fraud rules) and calls the template.

Fee *amounts* are computed by the client (business policy); the template
validates the structure. The ledger never rounds.

### 5.3 Guaranteeing every posted operation balances
Defence in depth, each layer independent:
1. **Types**: a `BalancedEntry` can only be constructed by a validator.
   Unbalanced data cannot reach the posting code ("parse, don't validate").
2. **Engine**: re-checks before writing.
3. **Database**: a `DEFERRABLE INITIALLY DEFERRED` constraint trigger rejects
   any commit where an entry's debits ≠ credits per currency.
4. **Verification**: a scheduled job checks global debits = credits and every
   account's totals = SUM(lines). A failure pages someone.

### 5.4 Preventing concurrent spending of the same balance
- `READ COMMITTED` + **pessimistic row locks**: `SELECT … FOR UPDATE` on every
  account in the entry, **in ascending account-id order** (prevents deadlocks).
- After locking, check the balance, then update totals and insert lines in the
  same transaction.
- Backstop: a `CHECK` on `accounts` so that a non-`allow_negative` account
  can never commit a balance below zero on its normal side.
- Bounded retry on `40P01` (deadlock) and `40001` (serialization failure).

Alternatives rejected: `SERIALIZABLE` everywhere (correct but retries spike
under contention on hot accounts); optimistic `version` checks (retry storms on
popular merchants). Re-evaluated with measurements in Week 10 and Week 35.

### 5.5 Preventing duplicate financial transactions
- **Idempotency key** per client, unique, written in the *same* transaction as
  the entry, with a SHA-256 fingerprint of the canonical request.
  Same key + same fingerprint → return the original result.
  Same key + different fingerprint → `409 Conflict`.
- **Business uniqueness** on top: one reversal per entry
  (`UNIQUE(reverses_entry_id)`), cumulative refunds ≤ original amount,
  one settlement per offline `(device_id, counter)`.

### 5.6 Reversals without modifying history
- A reversal is a **new entry** with every line mirrored, linked by
  `reverses_entry_id`. The original row is never updated; "is reversed" is
  derived from the existence of the reversal.
- A **refund** is a different thing: a new business operation that may be
  partial and may treat fees differently.
- An **adjustment** is a manual correction posted with a reason code under
  maker-checker approval, usually against a suspense or adjustment account.
- `UPDATE`/`DELETE` on journal tables are blocked by triggers *and* by role
  privileges.

### 5.7 Calculating and verifying balances efficiently
- **Stored running totals** (`debits_total`, `credits_total`) on the account
  row, updated in the posting transaction under the row lock: O(1) reads.
- Each **line stores the account totals after it**: you can read the balance at
  any point in history and verify the chain line by line.
- **Snapshots** at period close; verification only needs to scan lines since
  the last verified checkpoint.

Alternatives: derive on read with SUM (correct, O(n), unusable at scale);
materialized views (stale, so they can't be used for authorisation).

> Storing a balance does not make the system correct. The stored balance is
> correct only because it is updated atomically with the lines, under lock,
> and continuously re-proven against them.

### 5.8 Offline transaction synchronisation
Offline payments **cannot be treated as online database transactions**:
- While a device is offline, no central lock exists, so the ledger cannot
  prevent a double-spend at payment time.
- The device's clock, its storage and its owner may all be hostile.
- What arrives later is a *claim* that a payment happened, not a payment.

Correct architecture: **escrow + signed claims + deterministic settlement.**
1. While online, the ledger **reserves** an offline allowance: it posts from
   the wallet to a per-device escrow account and issues a **signed voucher**.
2. Offline, the device's secure hardware signs **receipts** with a monotonic
   counter and a hash chain.
3. On sync (from payer or payee, in any order, any number of times), the
   ledger verifies signature, voucher, counter and chain, then settles
   escrow → payee exactly once.
4. Duplicates are ignored, gaps wait, and a conflict (same counter, different
   content) freezes the device and goes to dispute.
5. Risk is bounded by the escrow amount and by per-trust-tier limits.
   Unspent escrow returns on expiry. Receipts arriving after the return go to
   suspense and are handled under a written loss policy.

Weeks 26–30 design, build and simulate this.

### 5.9 Reconciling cached balances against the ledger
Any balance stored outside the ledger (e.g. a client's wallet balance table) is a
**cache for display**. A recurring reconciliation compares it to the ledger.
On mismatch the **ledger wins**: the cache is refreshed and an alert is raised.
The ledger is never "fixed" to match a cache. Authorisation decisions read the
ledger, never the cache (Week 31).

### 5.10 Recovering from a database crash mid-operation
- Each posting is **one Postgres transaction**: it commits fully or not at all.
  `synchronous_commit = on` and `fsync = on` always, and ledger tables are
  never `UNLOGGED`.
- The idempotency record commits *with* the entry. A crash before commit means
  nothing happened, so the retry posts. A crash after commit means the retry
  returns the stored result.
- Client side: the client writes an **intent row** (with the idempotency key)
  before calling the ledger, and a worker retries unfinished intents. Lookup
  by idempotency key lets it learn the outcome after a lost response.
- Background jobs (hold expiry, settlement, outbox relay) are idempotent and
  resumable from database state alone. No money state lives only in memory.
- Disaster level: PITR from WAL archive, then run the full integrity
  verification before reopening (Week 42).

## 6. Standards: mandatory, useful, optional

| Standard | Status | Use |
|---|---|---|
| Double-entry principles | Mandatory | The whole model |
| ISO 4217 | Mandatory | Currency codes and minor units (ETB = 2 decimals) |
| PostgreSQL docs (MVCC, locking, isolation, WAL, partitioning) | Mandatory | Weeks 8–15, 36, 42 |
| OWASP ASVS + API Security Top 10 | Useful (adopt as checklist) | Weeks 22–25, 32, 50 |
| RFC 9457 (Problem Details) | Useful | API error format |
| ISO 20022 | Optional / future | Only when bank or switch integration is real (evaluated W40) |
| PCI DSS, EMV | Not assumed | Applicability depends on card design; confirm before claiming |
| National Bank of Ethiopia directives | **Unknown applicability**: research in Week 3 with a legal advisor | Licensing, stored value, offline limits, record retention |

No regulatory requirement is treated as binding until Week 3 research
establishes that it applies.
