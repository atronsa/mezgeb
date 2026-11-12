# Invariant Catalogue

> **Version:** v1, frozen on 2026-11-10 (step 1.03 exit gate). Any change to an
> invariant, including adding or removing one, needs an [ADR](adr/README.md).

An **invariant** is a statement that is true of the ledger at every moment,
whatever happens: crashes, retries, concurrent requests, bugs in a client. If
an invariant is ever false, the ledger is wrong and someone is alerted.

A **validation** is a check on one request, such as "the amount is a valid
number" or "the account exists". Validations reject bad input. Invariants
describe what the stored data must always look like. Most invariants are
protected by several validations and checks, at different layers, so a bug
in one layer is caught by another.

## Where invariants are enforced

| Layer | What it means | Example |
|---|---|---|
| **Type** | The Rust type system makes the bad state impossible to construct | A `BalancedEntry` can only be built by a validator ([ADR-0003](adr/0003-multi-line-journal-entries.md)) |
| **Engine** | The posting engine checks before writing, inside the transaction | Balance check under row lock ([ADR-0007](adr/0007-read-committed-ordered-row-locks.md)) |
| **DB** | A PostgreSQL constraint, trigger or role privilege rejects the commit | `UNIQUE(reverses_entry_id)` |
| **Job** | A scheduled verification job checks stored data after the fact and alerts | Account totals equal the sum of lines ([ADR-0012](adr/0012-stored-running-balances.md)) |
| **Protocol** | The offline protocol's signatures, counters and hash chains | A receipt with a bad signature is never settled ([ADR-0010](adr/0010-offline-escrow-model.md)) |

Each invariant lists its layers in order from earliest to last. Tests are
listed by ID in the [traceability matrix](#traceability-matrix).

## Invariants

### Balance

| ID | Invariant | Enforced at | Tests |
|---|---|---|---|
| INV-1 | Every entry has at least two lines, every line amount is greater than zero, and for each currency the entry's debits equal its credits. | Type, Engine, DB | T-01a, T-01b, T-01c |
| INV-2 | For each currency, the sum of all debits in the ledger equals the sum of all credits. | Job | T-02a, T-02b |
| INV-3 | Every line's currency is the currency of its account. Money never changes currency inside one line. | Type, Engine, DB | T-03a, T-03b |
| INV-4 | An entry is posted completely or not at all. No committed entry is missing lines, and no line exists without its entry. | Engine, DB | T-04a, T-04b |

### Balances and history

| ID | Invariant | Enforced at | Tests |
|---|---|---|---|
| INV-5 | Every account's stored `debits_total` and `credits_total` equal the sums of its debit and credit lines. | Engine, Job | T-05a, T-05b |
| INV-6 | Each line's stored account totals equal the account's totals after that line, in posting order. | Engine, Job | T-06a |
| INV-7 | Replaying the journal from empty reproduces every account balance exactly. | Job | T-07a, T-07b |
| INV-8 | An account without `allow_negative` never has a balance below zero on its normal side. | Engine, DB | T-08a, T-08b, T-08c |

### Immutability

| ID | Invariant | Enforced at | Tests |
|---|---|---|---|
| INV-9 | A posted entry or line is never updated or deleted. Corrections are new entries ([ADR-0011](adr/0011-append-only-history.md)). | DB (trigger and role privileges) | T-09a, T-09b |
| INV-10 | No entry is posted into a closed accounting period, and a closed period's balances never change. | Engine, DB | T-10a, T-10b |
| INV-11 | Every entry's hash equals the hash of its canonical content and the previous entry's hash. The chain verifies from the first entry to the last. | Engine, Job | T-11a, T-11b |
| INV-12 | The ledger's `recorded_time` strictly increases in posting order. A client's `event_time` is never used for ordering. | Engine, DB | T-12a |

### Uniqueness

| ID | Invariant | Enforced at | Tests |
|---|---|---|---|
| INV-13 | An idempotency key is applied at most once per client. A repeat with the same request returns the original result; a repeat with a different request is rejected ([ADR-0008](adr/0008-idempotency-in-posting-transaction.md)). | Engine, DB | T-13a, T-13b, T-13c |
| INV-14 | An entry is reversed at most once, and a reversal is the exact mirror of its original: same accounts, same amounts, opposite sides. A reversal can never itself be reversed; if a reversal was a mistake, the original is posted again as a new entry. | Engine, DB | T-14a, T-14b, T-14c |
| INV-15 | The total refunded against a payment never exceeds the payment's original amount. | Engine | T-15a, T-15b |

### Holds

| ID | Invariant | Enforced at | Tests |
|---|---|---|---|
| INV-16 | A hold is resolved at most once: captured, released or expired ([ADR-0004](adr/0004-holds-as-postings.md)). | Engine, DB | T-16a, T-16b |
| INV-17 | A hold's capture never exceeds its held amount. A partial capture releases the rest in the same entry. | Type, Engine | T-17a |
| INV-18 | A held sub-account's balance equals the sum of its open holds. When every hold is resolved, it is zero. | Engine, Job | T-18a |

### Offline

| ID | Invariant | Enforced at | Tests |
|---|---|---|---|
| INV-19 | The receipts settled against a voucher never exceed the voucher's escrow amount. | Engine, DB | T-19a, T-19b |
| INV-20 | A `(device_id, counter)` pair is settled at most once, whoever uploads it and however many times. | Engine, DB, Protocol | T-20a, T-20b |
| INV-21 | Two different receipts with the same `(device_id, counter)` are never both settled. The second one always raises a conflict and freezes the device. | Engine, Protocol | T-21a |
| INV-22 | Only receipts with a valid device signature, a known active voucher and an unbroken hash chain are settled. | Protocol, Engine | T-22a, T-22b |
| INV-23 | After a voucher expires and its unspent escrow is returned, its escrow sub-account is zero. | Engine, Job | T-23a |

### Control

| ID | Invariant | Enforced at | Tests |
|---|---|---|---|
| INV-24 | Lines post only to open posting accounts: never to a closed or frozen account, and never directly to a control account. | Engine, DB | T-24a, T-24b |
| INV-25 | Every adjustment has a reason code, a maker and a checker, and the maker and the checker are different people. | Engine, DB | T-25a |
| INV-26 | Every committed entry has exactly one signed ledger receipt, and a retry returns the identical receipt bytes (spec §7.2). | Engine, DB | T-26a, T-26b |

## Monitored conditions

These must hold at agreed checkpoints, not at every instant, so they are not
invariants. A failure raises an alert for a person to investigate.

| ID | Condition | Checked | Why not an invariant |
|---|---|---|---|
| MON-1 | **Safeguarding:** the customer funds account (1010) is at least the sum of customer, merchant and unidentified liabilities (2010, 2020, 2030, 2040, 2050, 9010). | Verification job, at least daily | The ledger does not know which accounts are customer money; the client's chart of accounts does. A compensation can briefly break it before funding arrives ([trial balance](accounting/trial-balance.md#balance-after-each-entry), E14). |
| MON-2 | Pending accounts (2020, 2040) have no balance older than the agreed limit. | Verification job | Payouts can be delayed by banks. A stuck balance needs investigation, not rejection. |
| MON-3 | Cached balances held by clients match the ledger. | Reconciliation (step 2.15) | The cache lives outside the ledger. On mismatch, the ledger wins ([ADR-0002](adr/0002-three-layer-separation.md)). |

## Traceability matrix

Every invariant has at least one planned test. Test kinds:

- **Unit:** a single function.
- **Property:** random inputs checked against the reference ledger (step 1.07).
- **DB:** runs against a real PostgreSQL database.
- **Concurrency:** many parallel requests.
- **Crash:** the process or database is killed mid-operation (step 1.15).
- **Simulation:** the deterministic simulator (`mezgeb-sim`).
- **Verify:** the verification job (`mezgeb-cli verify`).

| Test | Invariant | Kind | What it checks | Step |
|---|---|---|---|---|
| T-01a | INV-1 | Unit | An unbalanced, single-line or zero-amount draft cannot become a `BalancedEntry` | 1.06 |
| T-01b | INV-1 | Property | Every entry the engine accepts balances per currency | 1.07 |
| T-01c | INV-1 | DB | A raw insert of an unbalanced entry is rejected at commit by the deferred trigger | 1.08 |
| T-02a | INV-2 | Verify | Global debits equal credits after a full test suite | 1.13 |
| T-02b | INV-2 | Simulation | Global debits equal credits after every simulated step | 1.15 |
| T-03a | INV-3 | Unit | A line in ETB cannot be built for a USD account | 1.06 |
| T-03b | INV-3 | DB | A line whose currency differs from its account's is rejected | 1.08 |
| T-04a | INV-4 | Crash | Killing the database mid-posting leaves either the whole entry or nothing | 1.15 |
| T-04b | INV-4 | DB | No line exists without its entry (foreign key and orphan query) | 1.08 |
| T-05a | INV-5 | Property | Stored totals equal summed lines after random workloads | 1.09 |
| T-05b | INV-5 | Verify | Totals equal summed lines for every account | 1.13 |
| T-06a | INV-6 | Verify | Each line's stored totals follow from the previous line | 1.13 |
| T-07a | INV-7 | Property | Replaying the journal into the reference ledger gives identical balances | 1.07 |
| T-07b | INV-7 | Verify | Replay from the last snapshot matches stored balances | 2.05 |
| T-08a | INV-8 | Unit | A posting that would make a non-negative account negative is rejected | 1.09 |
| T-08b | INV-8 | Concurrency | Two parallel payments that together overspend: exactly one succeeds | 1.10 |
| T-08c | INV-8 | DB | The `CHECK` constraint rejects a direct update below zero | 1.08 |
| T-09a | INV-9 | DB | `UPDATE` and `DELETE` on journal tables fail, by trigger | 1.08 |
| T-09b | INV-9 | DB | The ledger's runtime role has no `UPDATE` or `DELETE` privilege | 1.08 |
| T-10a | INV-10 | Unit | Posting with an effective date in a closed period is rejected | 2.05 |
| T-10b | INV-10 | Verify | Closed-period balances match their snapshot | 2.05 |
| T-11a | INV-11 | Verify | The hash chain verifies from the first entry | 2.09 |
| T-11b | INV-11 | DB | Changing any stored entry byte is detected by verification | 2.09 |
| T-12a | INV-12 | Concurrency | `recorded_time` is strictly increasing under parallel posting (measured in 1.10; guaranteed by the single commit order of the hash chain) | 2.09 |
| T-13a | INV-13 | Unit | Same key and same request returns the original result | 1.11 |
| T-13b | INV-13 | Unit | Same key and different request returns a conflict | 1.11 |
| T-13c | INV-13 | Crash | A crash after commit, then a retry, posts nothing new | 1.15 |
| T-14a | INV-14 | DB | A second reversal of the same entry is rejected | 1.12 |
| T-14b | INV-14 | Property | Original plus reversal leaves every balance unchanged | 1.12 |
| T-14c | INV-14 | Unit | Reversing a reversal is rejected | 1.12 |
| T-15a | INV-15 | Unit | A refund above the remaining refundable amount is rejected | 2.04 |
| T-15b | INV-15 | Concurrency | Parallel partial refunds never exceed the original in total | 2.04 |
| T-16a | INV-16 | Unit | Capturing or releasing an already resolved hold is rejected | 1.14 |
| T-16b | INV-16 | Concurrency | Capture and expiry racing on one hold: exactly one wins | 1.14 |
| T-17a | INV-17 | Unit | A capture above the held amount is rejected; a partial capture releases the rest | 1.14 |
| T-18a | INV-18 | Verify | Each held sub-account equals the sum of its open holds | 1.14 |
| T-19a | INV-19 | Unit | A receipt that would exceed the voucher's escrow is rejected as over limit | 2.12 |
| T-19b | INV-19 | Simulation | No voucher ever settles more than its escrow, under any delivery order | 2.14 |
| T-20a | INV-20 | Unit | The same receipt uploaded twice returns duplicate the second time | 2.12 |
| T-20b | INV-20 | Simulation | Payer and payee uploading the same receipts in any order settle each once | 2.14 |
| T-21a | INV-21 | Simulation | A cloned device producing two receipts with one counter: one settles, the device freezes | 2.13 |
| T-22a | INV-22 | Unit | Receipts with a bad signature, unknown voucher or broken chain are rejected | 2.12 |
| T-22b | INV-22 | Simulation | Forged and replayed receipts never settle | 2.14 |
| T-23a | INV-23 | Unit | After expiry and return, the escrow sub-account is zero | 2.13 |
| T-24a | INV-24 | Unit | Posting to a closed, frozen or control account is rejected | 1.09 |
| T-24b | INV-24 | DB | A line to a closed account is rejected by the database | 1.08 |
| T-25a | INV-25 | Unit | An adjustment with the same maker and checker, or without a reason, is rejected | 1.12 |
| T-26a | INV-26 | Unit | Every committed entry has one receipt whose signature verifies | 2.09 |
| T-26b | INV-26 | Crash | A retry after a lost response returns identical receipt bytes | 2.09 |

## Mapping from the draft spec

The draft spec (§2) used a one-debit, one-credit transfer model. Its
invariants map to this catalogue as follows:

| Draft | Now | Change |
|---|---|---|
| INV-1 one debit, one credit per transfer | INV-1 | Restated for multi-line entries (ADR-0003) |
| INV-2 debits equal credits | INV-2 | Per currency instead of per ledger |
| INV-3 transfers are immutable | INV-9 | Same rule, for entries and lines |
| INV-4 transfer ID applied once | INV-13 | Keyed on the idempotency key (ADR-0006, ADR-0008) |
| INV-5 account balance flags | INV-8 | One `allow_negative` setting instead of two flags |
| INV-6 same ledger | INV-3 | Per line currency |
| INV-7 pending resolves once | INV-16 | Holds as postings (ADR-0004) |
| INV-8 replay | INV-7 | Unchanged |
| INV-9 to INV-11 offline | INV-19 to INV-21 | Unchanged |
| INV-12 closed periods | INV-10 | Unchanged |
| INV-13 ledger receipts | INV-26 | Per entry instead of per transfer |
