# Phase 3: Scale, Operations and Client Integration

**Goal:** measured performance, production operations, and a verified
migration of a first production client application onto the new ledger running in shadow mode.

**Milestone at the end of this phase:** M3 Shadow on live traffic (tag `v0.9.0-rc-shadow`)

| Step | Topic |
|---|---|
| 3.01 | [Performance Methodology and Realistic Workload](01-performance-methodology.md) |
| 3.02 | [Indexing and Query Optimisation](02-indexing-and-query-optimisation.md) |
| 3.03 | [Hot Accounts and Contention](03-hot-accounts-and-contention.md) |
| 3.04 | [Partitioning, Retention and Archival](04-partitioning-and-archival.md) |
| 3.05 | [Reporting and Financial Statements](05-reporting-and-financial-statements.md) |
| 3.06 | [Transactional Outbox and Event Integration](06-transactional-outbox.md) |
| 3.07 | [Merchant Settlement Automation](07-merchant-settlement.md) |
| 3.08 | [Multi-Currency, Multi-Tenant and ISO 20022 Evaluation](08-multi-currency-and-multi-tenant.md) |
| 3.09 | [Observability: Logs, Metrics, Dashboards, Alerts](09-observability.md) |
| 3.10 | [Backups, Point-in-Time Recovery and Disaster Recovery](10-backups-and-disaster-recovery.md) |
| 3.11 | [Deployment and CI/CD](11-deployment-and-ci-cd.md) |
| 3.12 | [Secrets, Key Management and Operational Security](12-secrets-and-key-management.md) |
| 3.13 | [Client Integration Design and Account Mapping](13-client-integration-design.md) |
| 3.14 | [Historical Replay and Migration Verification](14-historical-replay.md) |
| 3.15 | [Shadow Mode Implementation](15-shadow-mode.md) |
| 3.16 | [Shadow Operation, Discrepancy Resolution and Phase 3 Exit](16-shadow-operation-and-phase-3-exit.md) |

Steps run in order. Each one closes only when its **Exit Gate** is met and the
evidence is committed under `docs/evidence/`. Target dates are in the
[schedule](../README.md#schedule).
