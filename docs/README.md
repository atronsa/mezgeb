# Mezgeb Documentation

Start here. Each folder has its own `README.md` that lists its files in
reading order.

| Path | What it is | Read it when |
|---|---|---|
| [spec.md](spec.md) | The specification: invariants, data model, transfer semantics, offline sync, audit | You want to know what Mezgeb guarantees |
| [architecture.md](architecture.md) | System shape, stack choices and the ten architectural questions | You want to know why it is built this way |
| [accounting/](accounting/README.md) | Chart of accounts and worked journal entries for every money flow | You want to know how money is booked |
| [roadmap/](roadmap/README.md) | The engineering plan: 52 steps in four phases, milestones and schedule | You want to know what is being built next |

Planned folders, created by the roadmap step that first needs them:

| Path | Contents | Created in |
|---|---|---|
| `adr/` | Architecture decision records (`0001-…`) | [1.02](roadmap/phase-1-core-engine/02-architecture-decisions.md) |
| `evidence/` | Proof that each step's exit gate was met (`1.01-…`) | [1.01](roadmap/phase-1-core-engine/01-accounting-foundations.md) |

## Conventions

- File and folder names are lowercase `kebab-case`. A number prefix means
  reading order (`01-…`) or a stable ID (`0001-…`, step `1.01`).
- One file covers one topic. A file that grows past about 300 lines, or
  holds parts that make sense on their own, is split.
- Links between documents are relative, so they work on GitHub and locally.
