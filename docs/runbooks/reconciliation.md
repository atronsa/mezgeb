# Runbook: Daily Reconciliation

[Runbooks](README.md) · Code: [`crates/engine/src/recon/`](../../crates/engine/src/recon/mod.rs), [`crates/cli/src/recon.rs`](../../crates/cli/src/recon.rs) · Step: [2.15](../roadmap/phase-2-operations-and-security/15-reconciliation.md) · Decision: [ADR-0020](../adr/0020-reconciliation.md)

The ledger can be internally perfect and still disagree with the bank, a
client's cache or a device. Reconciliation finds every such disagreement as
a **break**. It never changes the ledger: the ledger is the system of
record, and a break is fixed by posting an entry or by refreshing the other
record, never by editing.

| | |
|---|---|
| Who | The on-call operator; a second person for any resolution over the operator's limit |
| When | Daily, once the partner bank's statement for the previous day is available (for example 06:00), after the period close |
| Takes | Seconds for a day; under two minutes each to import and to reconcile a million items (see the [evidence](../evidence/2.15-reconciliation.md)) |
| Commands | `mezgeb-cli recon import`, `recon run`, `recon offline`, `recon breaks` |
| Connects as | `mezgeb_verifier` (reads the ledger, writes only the reconciliation tables) |
| Resolves breaks through | The API, as an operator (`reconciliation:resolve`), so each is authorised and audited |
| Safe to repeat | Yes: an import skips lines already imported, and a run raises no break twice |

## 1. Import the statement

Download the customer funds account's statement for the previous day (or
since the last import) in the bank's CSV export, and import it into 1010:

```sh
export MEZGEB_DATABASE_URL=postgres://mezgeb_verifier:…@<host>/mezgeb
mezgeb-cli recon import 1010 statement-2026-11-16.csv --mapping /etc/mezgeb/bank.toml
```

```text
imported 412 of 412 lines into 1010 (0 already imported, 0 conflicts); run 0193…
```

- **A file that does not read** is refused whole, with its line number:
  `line 37: bad amount "1.000,5O"`. Nothing is imported. Fix the export (or
  the mapping, if the bank changed its format; see
  [statement-format.md](../reconciliation/statement-format.md)) and import
  again.
- **Overlapping files are fine.** Lines already imported are counted as
  "already imported" and skipped.
- **Exit status 1: a conflict.** The bank sent a line ID again with
  different content. The first copy stays; a `statement_conflict` break is
  open (see [Conflicts](#statement_conflict)).

## 2. Run the reconciliation

```sh
mezgeb-cli recon run 1010
mezgeb-cli recon offline
```

```text
run 0193… (external): 824 items, 410 matched, 3 pending; breaks: 1 new, 2 cleared, 1 open; 0.4 s
run 0193… (offline): 9531 items, 0 matched, 0 pending; breaks: 0 new, 0 cleared, 0 open; 0.2 s
```

- **Matched**: a statement line and a ledger line agree: by the bank's line
  ID (`bank_ref`), or by amount, reference and date within 3 days with no
  other candidate. Matching never guesses between two equal candidates.
- **Pending**: unmatched, but younger than 2 days. The client books a
  deposit after the bank shows it, so a fresh statement line without a
  ledger line is normal. It becomes a break if it is still unmatched in 2
  days.
- **Cleared**: an open break whose line has now matched (the bank's line
  arrived late, the client booked the deposit). Nothing to do.
- **Exit status 1**: breaks are open. Work through them below.

`--grace-days N` and `--window-days N` change the two periods for one run
(0 to 60). Use them only to investigate; the daily run uses the defaults.

## 3. Work through the breaks

```sh
mezgeb-cli recon breaks
```

```text
0193…a1 open missing_in_ledger difference 70.00: the bank's line B5 has no ledger line
0193…b2 open amount_mismatch difference -10.00: the bank's line B3 and the ledger line with its bank_ref differ
2 open breaks
```

The API gives the same list with every detail (`GET /v1/reconciliation/breaks`)
and each break by ID. The **difference** is always *the other record minus
the ledger*: positive when the bank has more than the ledger.

### Money breaks: put 1010 right first, then find the cause

For the four kinds about money in 1010, do not wait for the cause before
booking: **first make 1010 equal the bank** with a suspense entry, then
resolve the break with that entry. The difference then sits in its own
suspense item, where the ageing report keeps it visible until someone
explains it ([ADR-0020](../adr/0020-reconciliation.md) decision 7;
[entries E32–E35](../accounting/entries/reconciliation.md)).

| Kind | Means | Difference | Suspense entry |
|---|---|---|---|
| `missing_in_ledger` | The bank shows a movement the ledger does not | + money in, − money out | + : `deposit_unmatched.v1` (Dr 1010 / Cr `9010:sus-…`); − : `bank_debit_unmatched.v1` (Dr `9040:sbd-…` / Cr 1010) |
| `missing_in_statement` | The ledger shows a movement the bank does not | the opposite of the ledger line | as above, by sign |
| `amount_mismatch` | The same `bank_ref`, another amount | bank minus ledger | as above, by sign |
| `duplicate_statement_line` | A second statement line with the same date, amount and reference, and no ledger line of its own | its amount | as above: if the bank really booked it twice, the money is there |

1. **Book the suspense entry** through the API, in a new suspense item,
   for exactly the difference, as a client posts any operation (with an
   `Idempotency-Key`). For a difference of −10.00:

   ```http
   POST /v1/operations/bank_debit_unmatched.v1
   Idempotency-Key: recon-0193…b2
   {"accounts": {"bank_debit_item": "9040:sbd-2026-11-17-01", "customer_funds": "1010"},
    "amounts": {"amount": {"amount": "10.00", "currency": "ETB"}}}
   ```

   Open the item first (`POST /v1/accounts`, reference `9040:sbd-…` or
   `9010:sus-…`); each item is used once.
2. **Resolve the break** with that entry:

   ```http
   POST /v1/reconciliation/breaks/0193…b2/resolve
   {"entry_id": "<the entry's id>"}
   ```

   The ledger checks that the entry was booked by one of the two templates,
   that its one line in 1010 equals the difference in amount and
   direction, and that it has resolved no other break. Otherwise
   `409 break_not_closable`, with the reason.
3. **Find the cause and close the suspense item**, from the ageing report
   (`GET /v1/reports/ageing?code=9010&code=9040`):

   | Item | Cause found | Close with |
   |---|---|---|
   | `9010:sus-…` | The owner of the money | `suspense_resolve.v1` to their wallet or merchant balance (E20) |
   | `9040:sbd-…` | A bank charge | `bank_debit_resolve.v1`, cause 5020 (E33) |
   | `9040:sbd-…` | A deposit booked twice, or money credited that never arrived | `bank_debit_resolve.v1`, cause the wallet or merchant balance (E35) |
   | `9040:sbd-…` | The bank's mistake, since corrected (its statement shows the money back) | `bank_debit_resolve.v1`, cause 1010; the bank's correcting line is itself matched by booking it with its `bank_ref` |

   An item always closes whole. If the wallet no longer holds the money,
   the posting is refused and the item stays open and ages: escalate it as
   below.

A money break is never "explained": the API refuses it. It closes only
when a later run matches it, or a suspense entry resolves it.

### `statement_conflict`

The bank sent the same line ID twice with different content. The stored
copy stays and is the one matched. Ask the bank which copy is right:

- **The first is right** (the bank's export was corrected): explain the
  break, quoting the bank's answer.
- **The second is right**: explain the break, then treat the difference as
  a money break: book the suspense entry for it, so 1010 equals the
  corrected statement, and record its entry ID in the explanation.

```http
POST /v1/reconciliation/breaks/0193…c3/explain
{"reason": "Bank ticket 4471: the 16 Nov export was re-issued; the first copy is right."}
```

### `stale_cache`

A client's cached balance disagreed with the ledger's at the same point in
posting order. The ledger is right: the client refreshes its cache from the
balance the API returned, and its next comparison clears the break. If it
stays open for more than a day, contact the client: its cache is missing
or replaying events. Explain the break once the client has fixed it.

### Offline breaks

| Kind | Means | Do |
|---|---|---|
| `missing_receipts` | A device reported counters the ledger never received receipts for, and their voucher has ended, so they can no longer be paid | Ask the device owner's client which payees hold the receipts. They are the payees' loss under the protocol's deadline ([offline runbook](offline.md)); explain the break with what was found. The break clears by itself if the receipts are uploaded, even late |
| `counter_behind` | A device reported a lower counter than one it had signed: reset, restored from a backup, or cloned | Freeze the device (`mezgeb-cli offline device freeze`) and follow the offline runbook's device review; explain the break with the outcome |
| `receipt_entry_mismatch` | A settled receipt whose entry does not debit its amount or pay its payee | **A defect.** Do not explain it away: open an incident, keep the break open, and escalate to engineering |
| `escrow_mismatch` | An escrow that does not hold its voucher's unspent value (also `mezgeb-cli offline check`) | **A defect.** As above |

## Ageing and escalation

| Age of an open break or suspense item | Action |
|---|---|
| Under 1 day | Normal: being worked |
| 1 to 3 days | Chase the bank or the client |
| 3 to 7 days | Alert the finance lead |
| 7 days and over | Escalate; for a 9010 item consider returning the money to the sender; for a 9040 item, report the shortfall in the safeguarding check |

While a 9040 item is open, 1010 is lower than the customer balances it
backs by that amount: the safeguarding check shows it until it is closed
([chart of accounts](../accounting/chart-of-accounts.md#account-groups-and-the-safeguarding-check)).

## Scheduling it

Run the three commands from the scheduler that runs the period close, after
it, once the statement file has been fetched. Alert on exit status 1 of
`recon run` and `recon offline`, and always on status 2 (could not run).

```sh
mezgeb-cli recon import 1010 "$STATEMENT" --mapping /etc/mezgeb/bank.toml || alert "statement conflict or bad file"
mezgeb-cli recon run 1010 || alert "reconciliation breaks open for 1010"
mezgeb-cli recon offline || alert "offline reconciliation breaks open"
```

## Checks

```sql
-- The latest runs and what they found.
SELECT kind, as_of, items, matched, pending, breaks_new, breaks_cleared, breaks_open,
       runtime_micros / 1000 AS ms
  FROM recon_runs ORDER BY recorded_at DESC LIMIT 10;

-- Open breaks by kind and age.
SELECT kind, count(*), min(found_at) AS oldest
  FROM recon_breaks WHERE status = 'open' GROUP BY kind ORDER BY oldest;

-- Statement lines not yet matched, oldest first.
SELECT line_ref, booking_date, amount_minor, reference
  FROM recon_items WHERE match_rule IS NULL ORDER BY booking_date LIMIT 50;
```
