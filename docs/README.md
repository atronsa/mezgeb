# Mezgeb Documentation

Start here. Each folder has its own `README.md` that lists its files in
reading order.

| Path | What it is | Read it when |
|---|---|---|
| [spec.md](spec.md) | The specification: invariants, data model, transfer semantics, offline sync, audit | You want to know what Mezgeb guarantees |
| [invariants.md](invariants.md) | Every rule the ledger must never break, where it is enforced and how it is tested | You want to know what "correct" means |
| [threat-model.md](threat-model.md) | Who might attack the ledger, what they want, and the control and owner for each threat | You want to know how the ledger is protected |
| [regulatory.md](regulatory.md) | Which laws may apply to an operator, as questions with sources and status | You want to know what regulation may require |
| [architecture.md](architecture.md) | System shape, stack choices and the ten architectural questions | You want to know why it is built this way |
| [adr/](adr/README.md) | Architecture decision records: one binding decision per file, with alternatives | You want the reason behind a table, crate or rule |
| [api/](api/README.md) | The REST API contract (`openapi/mezgeb.v1.yaml`), its conventions, error catalogue and OWASP review | You integrate with the ledger or change the API |
| [db-schema.md](db-schema.md) | PostgreSQL tables, the constraint behind each invariant, roles, and how to run the database tests | You want to know what the database itself guarantees |
| [accounting/](accounting/README.md) | Chart of accounts and worked journal entries for every money flow | You want to know how money is booked |
| [runbooks/](runbooks/README.md) | Step-by-step operator procedures, such as the daily period close | You operate a ledger |
| [roadmap/](roadmap/README.md) | The engineering plan: 52 steps in four phases, milestones and schedule | You want to know what is being built next |
| [evidence/](evidence/README.md) | Proof that each step's exit gate was met | You want to check that a step is really done |

## Conventions

- File and folder names are lowercase `kebab-case`. A number prefix means
  reading order (`01-…`) or a stable ID (`0001-…`, step `1.01`).
- One file covers one topic. A file that grows past about 300 lines, or
  holds parts that make sense on their own, is split.
- Links between documents are relative, so they work on GitHub and locally.
