# Contributing to Mezgeb

Thank you for your interest. Mezgeb is a double-entry ledger, so the bar for
correctness is high: every change has to keep the books right, under
concurrency and crashes, and prove it with tests.

By taking part you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
To report a security issue, see [SECURITY.md](SECURITY.md), not a public
issue.

## Ways to help

- **Read the design and tell us what is wrong.** The
  [specification](docs/spec.md), the [architecture](docs/architecture.md),
  the [invariants](docs/invariants.md) and the
  [decision records](docs/adr/README.md) are where mistakes are cheapest to
  fix.
- **Accounting review.** If you know bookkeeping, the
  [worked entries](docs/accounting/) and the chart of accounts need expert
  eyes.
- **Report bugs**, with the steps to reproduce them.
- **Code**, following the [roadmap](docs/roadmap/README.md). Steps run in
  order, and each one ends with evidence under [docs/evidence/](docs/evidence/).

**Before starting anything large, open an issue** so we can agree on the
approach. A change that does not fit the roadmap or an accepted ADR is
hard to merge, however good the code.

## Setting up

You need:

- Rust 1.96 (`rust-toolchain.toml` selects it)
- Docker, for PostgreSQL 18

```sh
git clone https://github.com/atronsa/mezgeb.git
cd mezgeb
docker compose up -d
export MEZGEB_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:55432/postgres
cargo test --workspace
```

Without `MEZGEB_TEST_DATABASE_URL`, the database tests are skipped. In CI
they always run. `docker compose down` stops PostgreSQL and deletes its
data.

## Before you open a pull request

CI runs these, and all must pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace            # with MEZGEB_TEST_DATABASE_URL set
cargo run -p mezgeb-sim -- --seed 1 --ops 10000
```

The rules the code follows:

- **No `unsafe`, and no `unwrap()`.** The workspace forbids `unsafe` and
  denies `clippy::unwrap_used`. Return errors instead; tests return
  `Result` too.
- **Money is `i64` minor units** ([ADR-0005](docs/adr/0005-money-as-i64-minor-units.md)),
  with checked arithmetic. Never floating point.
- **History is append-only** ([ADR-0011](docs/adr/0011-append-only-history.md)).
  Never update or delete a posted entry; corrections are new entries.
- **Every rule has a test, and money rules have a database test.** A
  change to posting, holds, reversals or verification needs a test against
  PostgreSQL (`crates/engine/tests/`), not only against the reference
  ledger.
- **The reference ledger is the oracle.** `MemLedger` in `crates/core` must
  give the same answer as PostgreSQL. If you change a rule, change both,
  and the differential test and the simulator check that they agree.

## Changing the design

- **An invariant** ([docs/invariants.md](docs/invariants.md)) changes only
  through a new ADR.
- **An accepted ADR** is not edited to change its decision. Write a new ADR
  that supersedes it ([how](docs/adr/README.md)).
- **The database schema** changes only through a new migration in
  `migrations/`, with its `down` script. Never edit a migration that has
  been merged.

## Pull requests

- **One topic per pull request.** Smaller is easier to review.
- **The title is a [Conventional Commit](https://www.conventionalcommits.org/)**
  starting with a lowercase letter, such as `feat: add holds` or
  `fix(engine): retry on deadlock`. A check enforces it. Pull requests are
  squash-merged, and the title becomes the commit on `main`.
- **Say what changed and how you tested it.** The pull request template
  asks for both.
- **Update the docs in the same pull request** when behaviour changes.

## Test hooks

The engine's `test-hooks` feature injects faults for the failure tests and
the simulator. It must never be in a build that handles real money. Cargo
merges features across a workspace build, so build the service on its own:

```sh
cargo build --release -p mezgeb-cli
```

## Licence

Mezgeb is licensed under the [Apache License, Version 2.0](LICENSE). Unless
you say otherwise, any contribution you submit is licensed under the same
terms, as section 5 of the licence says. There is no separate contributor
agreement to sign.
