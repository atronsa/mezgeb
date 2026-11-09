# Phase 1: Accounting Model and Core Engine

**Goal:** a correct, concurrent, idempotent posting engine on PostgreSQL,
proven by property, concurrency and failure tests, with no public API yet.

**Why no API yet:** an API frozen before the accounting model is understood
turns modelling mistakes into compatibility promises.

**Milestone at the end of this phase:** M1 Engine correct (tag `v0.1.0-engine`)

| Step | Topic |
|---|---|
| 1.01 | [Accounting Foundations and Worked Examples](01-accounting-foundations.md) |
| 1.02 | [Architecture Decision Records and Repository Restructure](02-architecture-decisions.md) |
| 1.03 | [Invariant Catalogue, Threat Model v0, Regulatory Applicability](03-invariants-threat-model-and-regulation.md) |
| 1.04 | [Money, Currency and Identifier Types](04-money-and-identifier-types.md) |
| 1.05 | [Chart of Accounts and the Account Model](05-chart-of-accounts.md) |
| 1.06 | [Journal Entries and Posting Rules (Pure Core)](06-journal-entries-and-posting-rules.md) |
| 1.07 | [In-Memory Reference Ledger (the Oracle)](07-reference-ledger-oracle.md) |
| 1.08 | [PostgreSQL Schema v1, Constraints and Roles](08-postgresql-schema.md) |
| 1.09 | [Atomic Posting with SQLx](09-atomic-posting.md) |
| 1.10 | [Concurrency Control and Concurrent Spending](10-concurrency-control.md) |
| 1.11 | [Idempotency and Exactly-Once Effect](11-idempotency.md) |
| 1.12 | [Reversals and Adjustments](12-reversals-and-adjustments.md) |
| 1.13 | [Balance Verification and Integrity Checking](13-balance-verification.md) |
| 1.14 | [Holds and Two-Phase Operations](14-holds-and-two-phase-operations.md) |
| 1.15 | [Failure Injection and Crash Recovery for the Core](15-failure-injection-and-crash-recovery.md) |
| 1.16 | [Phase 1 Consolidation, Baseline Benchmark and Exit Review](16-phase-1-exit-review.md) |

Steps run in order. Each one closes only when its **Exit Gate** is met and the
evidence is committed under `docs/evidence/`. Target dates are in the
[schedule](../README.md#schedule).
