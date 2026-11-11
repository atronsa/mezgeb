# Mezgeb

**Double-entry ledger with offline sync and cryptographically signed transactions.**

An open-source financial ledger written in Rust on PostgreSQL. It is the
system of record for money: what every account holds, how it got there, who
caused each movement, and proof that the books balance.

It is application-agnostic: it knows nothing about any product's users, cards
or QR codes. Any application that moves money can use it, including
offline-first payment platforms.

*Mezgeb* (መዝገብ) is the Amharic word for a ledger or register.

> **Status: early development. Not production ready.**
> The design is being written first. See the [roadmap](docs/roadmap/README.md).
> Do not use this to hold real money yet.

---

## Why another ledger?

Most ledgers assume every payment can reach the server at the moment it
happens. In many places, including much of Ethiopia, that is not true. This
ledger treats offline payments as a first-class case:

- Funds for offline use are **reserved in escrow** while the device is online.
- Each offline payment is a **signed, hash-chained receipt** produced by the
  device's secure hardware.
- When either side syncs, in any order and any number of times, the ledger
  verifies the receipt and **settles it exactly once**.
- A cloned or tampered device is detected and frozen. Losses are bounded by
  the escrow amount.

## Features

**Accounting**
- Genuine double-entry bookkeeping with multi-line journal entries.
- Chart of accounts with account types and normal balances.
- Accounting periods, closing and balance snapshots.
- Trial balance, general ledger, balance sheet and income statement.

**Correctness**
- Every entry balances. This is enforced by Rust types, by the engine and by
  the database.
- Posted entries are immutable. Corrections are reversals or adjustments,
  never edits.
- Idempotent posting: retries never double-charge.
- Safe under concurrency: the same balance cannot be spent twice.
- Integer minor units only. No floating-point money and no silent rounding.

**Operations**
- Holds (authorise now, capture or release later).
- Deposits, withdrawals, transfers, merchant payments, fees, refunds,
  disputes and settlement.
- Clearing and suspense accounts for money in flight.
- Reconciliation against bank statements and external balance caches.

**Security and audit**
- Hash-chained journal with signed daily checkpoints (tamper-evident).
- Cryptographically signed offline vouchers and receipts (Ed25519).
- Role-based access control with default deny and maker-checker approval
  for manual money movements.
- Append-only audit log for every change.

Most of these are planned, not built yet. The roadmap tracks progress.

## Design principles

1. **Invariants first.** Every rule is written down
   ([docs/spec.md](docs/spec.md)) and tested before it is relied on.
2. **The database is the last line of defence.** Constraints, triggers and
   role privileges enforce the core rules even if application code is wrong.
3. **Verify, don't trust.** Stored balances are continuously re-proven against
   journal entries.
4. **Deterministic core.** The accounting engine does no I/O, so it can be
   tested with randomised simulations of millions of operations.
5. **No unnecessary infrastructure.** One service and PostgreSQL. No message
   broker, no cache.

## Architecture

```
  Client application (any payment app)
              │  REST + idempotency keys
              ▼
  ┌─────────────────────────────────────┐
  │ api      Axum, authn/authz          │
  │ ops      versioned posting templates│
  │ offline  vouchers, receipts         │
  │ recon / reports / audit / jobs      │
  │ engine   post, hold, reverse        │
  │ core     pure domain, no I/O        │
  └─────────────────┬───────────────────┘
                    ▼
               PostgreSQL
```

Full details and the reasoning behind each decision are in
[docs/architecture.md](docs/architecture.md).

## Repository layout

| Path | Contents |
|---|---|
| `crates/core` | Pure domain, no I/O: money, accounts, entries, validation |
| `crates/engine` | Posting engine on PostgreSQL: entries, holds, reversals |
| `crates/ops` | Versioned posting templates, one per business event |
| `crates/offline` | Vouchers, receipts, device chains, settlement rules |
| `crates/api` | The REST service (Axum) |
| `crates/sim` | Deterministic simulator that checks every invariant |
| `crates/cli` | Operator command line: verification, imports |
| `charts/` | Example charts of accounts, loaded as data (`wallet-provider.toml`) |
| `migrations/` | PostgreSQL schema migrations ([docs/db-schema.md](docs/db-schema.md)) |
| `db/init/` | Database roles, created once per PostgreSQL cluster |
| `compose.yaml` | Local PostgreSQL for development and database tests |
| `docs/` | Documentation: start at [docs/README.md](docs/README.md) |
| `docs/spec.md` | Specification |
| `docs/architecture.md` | Architecture overview |
| `docs/adr/` | Architecture decision records: why each decision was made |
| `docs/accounting/` | Accounting model: chart of accounts and worked journal entries |
| `docs/roadmap/` | Engineering roadmap: 52 steps in four phases |

Folders use short names (`crates/engine`); packages use the `mezgeb-`
prefix (`mezgeb-engine`), which is the name used with `cargo -p` and in
`use` statements (`mezgeb_engine`). Crates depend only on the layers below
them: `core` ← `engine` ← `ops` ← `api` ([ADR-0002](docs/adr/0002-three-layer-separation.md)).

## Getting started

Requirements: Rust 1.96+, Docker (for the database tests).

```sh
git clone <repo-url>
cd mezgeb
cargo build --workspace
cargo test --workspace
cargo run -p mezgeb-sim -- --seed 1 --steps 10000
```

The database tests are skipped unless a database is available. To run them:

```sh
docker compose up -d
MEZGEB_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:55432/postgres \
    cargo test -p mezgeb-engine
```

The posting engine works on PostgreSQL (`mezgeb-engine`); there is no API
service to run yet (steps 2.06–2.07).

## Roadmap

| Milestone | Scope |
|---|---|
| M1 Engine | Balanced, idempotent, concurrent-safe posting; holds; reversals; verification |
| M2 Functional | Financial operations, REST API, access control, audit chain, offline sync, reconciliation |
| M3 Shadow | Performance, operations, running in shadow mode beside an existing production ledger |
| M4 Release candidate | Security assessment, runbooks, production-readiness evidence |

See [docs/roadmap/README.md](docs/roadmap/README.md).

## Contributing

The project is in its design phase. Feedback on the
[specification](docs/spec.md) and the
[architecture](docs/architecture.md) is the most useful contribution
right now. Please open an issue before starting large changes.

## Security

Please do not report security issues in public issues. A `SECURITY.md` with
the disclosure process will be added before the first release.

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
