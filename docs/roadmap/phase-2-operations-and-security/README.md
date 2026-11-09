# Phase 2: Financial Operations, API, Security and Offline

**Goal:** real money operations as versioned posting templates, a secured REST
API, tamper-evident audit, offline escrow settlement and reconciliation.

**Milestone at the end of this phase:** M2 Functional and secured (tag `v0.2.0-functional`)

| Step | Topic |
|---|---|
| 2.01 | [Operations Layer and Posting Templates](01-posting-templates.md) |
| 2.02 | [Deposits, Withdrawals, Clearing and Suspense Accounts](02-deposits-withdrawals-and-clearing.md) |
| 2.03 | [Transfers, Merchant Payments and Fees](03-transfers-payments-and-fees.md) |
| 2.04 | [Refunds and Disputes](04-refunds-and-disputes.md) |
| 2.05 | [Accounting Periods, Closing and Trial Balance](05-accounting-periods-and-closing.md) |
| 2.06 | [REST API Design (Contract First)](06-rest-api-design.md) |
| 2.07 | [REST API Implementation (Axum)](07-rest-api-implementation.md) |
| 2.08 | [Authentication, Authorization and Privilege Separation](08-authentication-and-authorization.md) |
| 2.09 | [Audit Logging and Tamper Evidence](09-audit-logging.md) |
| 2.10 | [Offline Architecture and Protocol Design (Design Only)](10-offline-protocol-design.md) |
| 2.11 | [Device Registry and Voucher Issuance](11-device-registry-and-vouchers.md) |
| 2.12 | [Receipt Ingestion and Settlement](12-receipt-ingestion-and-settlement.md) |
| 2.13 | [Gaps, Conflicts, Expiry, Rejections and Disputes](13-offline-conflicts-and-expiry.md) |
| 2.14 | [Offline Deterministic Simulation](14-offline-simulation.md) |
| 2.15 | [Reconciliation (Internal, External, Offline)](15-reconciliation.md) |
| 2.16 | [Phase 2 Security Review and Exit](16-phase-2-security-review-and-exit.md) |

Steps run in order. Each one closes only when its **Exit Gate** is met and the
evidence is committed under `docs/evidence/`. Target dates are in the
[schedule](../README.md#schedule).
