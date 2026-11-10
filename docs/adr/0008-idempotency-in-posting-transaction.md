# 0008. Idempotency is recorded in the posting transaction

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-10 |
| Answers | [Architecture §5.5](../architecture.md#55-preventing-duplicate-financial-transactions), [§5.10](../architecture.md#510-recovering-from-a-database-crash-mid-operation) |
| Implemented in | 1.11 (idempotency), 1.15 (failure injection and crash recovery), 3.13 (client intent table) |

## Context

Networks fail. A client may send a payment, lose the response, and send it
again. The server may crash after writing the entry but before replying. In
every case, the payment must happen **exactly once**.

Because the ledger is a separate service (ADR-0001), the client cannot
commit its own state and the posting in one transaction. Retrying must
therefore always be safe.

## Decision

Every write carries a client-chosen **idempotency key**, unique per client.

- The ledger stores the key with a **SHA-256 fingerprint** of the canonical
  request, **in the same database transaction** as the entry.
- **Same key, same fingerprint:** return the original result. Nothing is
  posted again.
- **Same key, different fingerprint:** reject with `409 Conflict`.
- Clients can look up the result of a write by its idempotency key.

On top of the key, the ledger enforces **business uniqueness**:

- One reversal per entry (`UNIQUE(reverses_entry_id)`).
- Total refunds of a payment never exceed the original amount.
- One settlement per offline `(device_id, counter)`.

**Crash recovery follows from this:**

- Each posting is **one PostgreSQL transaction**. It commits fully or not at
  all. `synchronous_commit` and `fsync` stay on, and ledger tables are never
  `UNLOGGED`.
- A crash before commit means nothing happened, so the retry posts. A crash
  after commit means the retry returns the stored result.
- The client writes an **intent row** with the idempotency key before calling
  the ledger. A worker retries unfinished intents.
- Background jobs (hold expiry, settlement, outbox relay) are idempotent and
  resume from database state alone. No money state lives only in memory.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Key stored in the posting transaction (chosen)** | Exactly once, even across crashes. No extra system to keep consistent. | Idempotency records must be kept and eventually expired. Every write pays for one extra insert. |
| Key stored in Redis or another cache | Fast lookups. | Cannot commit atomically with the entry. A crash between the two causes a double post or a lost key. |
| No keys: deduplicate by business fields (amount, accounts, time) | Nothing for clients to do. | Two genuine identical payments look like one. Unsafe. |
| Client-generated entry IDs as the key | One field instead of two. | Mixes identity with retry safety (ADR-0006). No fingerprint, so a changed retry is not detected. |

## Consequences

**Positive:**

- Retries are always safe, so clients can retry aggressively.
- Crash recovery needs no special code path in the ledger.

**Negative:**

- Clients must generate and persist keys, and keep an intent table.
- A retention period for idempotency records must be chosen (step 1.11).

**Makes out of date:** spec §4.7, which keys idempotency on the transfer ID.
Rewritten in step 1.11.
