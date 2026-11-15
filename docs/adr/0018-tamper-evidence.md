# 0018. Entries are hash-chained and signed in the posting transaction; signed checkpoints are published to clients and to a file

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-15 |
| Answers | Spec §7 (audit and tamper evidence); INV-11, INV-26; TM-11, TM-16 |
| Implemented in | 2.09 (audit logging and tamper evidence) |

## Context

Triggers and role privileges stop the service from changing history
(INV-9), but a database superuser can still rewrite or delete rows without
a trace. Auditors, clients and regulators need **evidence** that this did
not happen: not proof that it cannot happen, which no database can give,
but a way to detect it afterwards.

## Decision

1. **A hash chain over the journal** (INV-11), in a separate append-only
   table, `entry_chain`, with one row per entry:
   - The row holds its position, the previous hash, the entry's hash, a
     signature and a key ID.
   - The hash is SHA-256 over a fixed, versioned byte encoding of the
     entry, its lines in order, and the previous hash. Nothing in it is
     JSON, so the bytes never depend on a library.
   - The row is written at the end of the posting transaction, under a lock
     on a single `chain_head` row. Chain order is therefore commit order.
   - `journal_entries` is unchanged.
2. **A ledger receipt per entry** (INV-26, spec §7.2).
   - The chain row's Ed25519 signature over the hash is the receipt. The
     API returns it as the entry's `receipt`.
   - Receipts are stored, so a retry returns identical bytes.
   - Public keys are listed by key ID at `GET /v1/keys`. Retired keys stay
     listed, so old receipts remain verifiable.
3. **Signed checkpoints** at each period close:
   - A checkpoint is the chain's position and hash at the close, signed.
   - **Published to clients and to a file.** Clients fetch checkpoints
     (`GET /v1/checkpoints`) and keep copies. The operator exports each one
     to a file outside the database (`mezgeb-cli checkpoint export`), on
     other storage.
   - A deleted or rewritten tail then disagrees with a checkpoint someone
     else holds.
4. **Audit events** (`audit_events`):
   - One event per API mutation and per refusal: actor, client, action,
     target, request ID, outcome.
   - For changes to configuration (not to the ledger), the event records
     before and after.
   - Reads across clients (`accounts:read_all`) are recorded too (TM-16).
   - Events are append-only and hash-chained, with their own head.
5. **Verification.** `mezgeb-cli verify --chain` recomputes both chains
   from the first row and checks every signature and every checkpoint.
6. **The signing key** is an Ed25519 seed read from a file
   (`MEZGEB_SIGNING_KEY_FILE`) until step 3.12 moves it to a key service.
   It is never stored in the database.

## What it proves, and what it does not

- **Proves.** Any change to a chained entry, line or audit row breaks the
  chain. Any rewrite or deletion before the last published checkpoint
  disagrees with that checkpoint. A client holding a receipt can prove the
  ledger recorded that entry.
- **Does not prove.** An entry deleted *after* the last checkpoint, with
  the chain rewritten by someone who also holds the signing key, goes
  undetected until clients compare receipts. Nor does it prove that a
  posting was right, only that it was not changed afterwards.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Chain table written in the posting transaction (chosen)** | Every committed entry is chained at once; chain order is commit order | Postings serialise on the chain head for the last part of the transaction; measured, and recorded for step 3.03 |
| Hash columns on `journal_entries` | One table | The row is written before its lines, and history may not be updated, so the hash could not cover the lines |
| A background job chaining committed entries | No lock in posting | Sequence order is not commit order (INV-12): the job must wait out a settle time, and an entry is unchained meanwhile |
| Publishing checkpoints to a public Git repository | Anyone can check them | Depends on a third party; clients still need their own copies |
| Write-once cloud storage | Strong immutability | Needs a cloud provider; possible later as one more file target |

## Consequences

**Positive:**

- Silent tampering by a superuser becomes detectable (TM-11).
- Every client can hold its own proof: receipts for its entries, and
  checkpoints for the whole ledger.

**Negative:**

- Postings serialise briefly on the chain head. The cost is measured in
  this step, and sharding the chain is left to step 3.03.
- A crash between an API write committing and its audit event being
  written loses that one audit event. The entry itself is still in the
  chain.
- The signing key in a file is only as safe as the server, until step 3.12.

**Makes out of date:** the `receipt: null` placeholder in the contract.
