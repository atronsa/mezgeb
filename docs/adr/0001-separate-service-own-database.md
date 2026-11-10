# 0001. The ledger is a separate service with its own database

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-10 |
| Answers | [Architecture §3](../architecture.md#3-shape-of-the-system-a-modular-monolith-behind-its-own-boundary), [§5.1](../architecture.md#51-modular-monolith-initially) |
| Implemented in | 1.08 (schema and roles), 2.08 (authentication and authorisation) |

## Context

The ledger is the system of record for money. Whoever can write to its
tables can create money. Mezgeb is also meant to be reused by any wallet or
payment platform, not built into one of them.

The application that uses the ledger (its *client*) has many features, many
dependencies and a large attack surface: user sign-up, KYC, QR codes, cards,
fees, notifications. If that application is compromised, the ledger must
still be safe.

## Decision

Mezgeb runs as **one deployable service with its own PostgreSQL database and
its own database roles**. Clients talk to it only through its API. No client
has credentials for the ledger database.

Inside the service, the code is split into crates (a *modular monolith*).
The crates call each other through Rust interfaces, not over the network.
The service is split into separate services only if a measured bottleneck
requires it; none is expected within the roadmap.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Separate service, own database (chosen)** | Hard security boundary: the ledger's roles can only insert entries, and client credentials cannot touch ledger tables. Testable, releasable and reusable on its own. | Business state and postings live in two databases, so cross-system consistency has to be engineered (ADR-0008). One more service to run. |
| Library embedded in the client's database | One transaction covers business state and the posting, which is the simplest consistency. | The client's credentials can write to ledger tables, so a compromised client server can write money. Not reusable as a product. |
| Several ledger microservices (accounts, postings, holds…) | Independent scaling of each part. | Distributed transactions inside the money path. No benefit at the expected scale. |

## Consequences

**Positive:**

- Database privileges become a security control: the ledger's runtime role
  cannot `UPDATE` or `DELETE` journal rows (ADR-0011).
- The ledger can be released, versioned and tested independently of any
  client.

**Negative:**

- A client cannot commit its business state and a posting in one
  transaction. It must use idempotency keys and an intent table to retry
  safely (ADR-0008).
- Every posting is a network call. This is accepted: the latency targets in
  spec §11 assume it.

**Design rule:** the `core` crate does no I/O, and the engine is written
against `&mut PgConnection`. The engine could therefore be embedded later
without a rewrite, if a client ever needs it.
