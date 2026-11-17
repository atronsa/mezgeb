# 0020. Reconciliation matches without guessing, the ledger wins, and breaks are resolved by suspense entries

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-17 |
| Answers | [Architecture §5.9](../architecture.md#59-reconciling-cached-balances-against-the-ledger); [ADR-0002](0002-three-layer-separation.md) (the ledger wins over caches) |
| Implemented in | 2.15 (reconciliation) |

## Context

The ledger checks itself: every entry balances, stored running totals equal
the sum of the lines, and the chain of entries is signed (steps 1.13, 2.05,
2.09). None of that shows that it agrees with the other records of the same
money:

- **External.** 1010, the customer funds account, must equal the partner
  bank's statement ([clearing and suspense](../accounting/clearing-and-suspense.md),
  rule 1). A deposit booked twice, a payout the bank never sent, or a bank
  charge nobody booked leaves the ledger internally perfect and wrong.
- **Internal.** A client platform keeps wallet balances in its own tables to
  show them quickly. A missed or replayed event makes that cache disagree
  with the ledger.
- **Offline.** A device reports its last counter each time it asks for a
  voucher (step 2.11). Receipts it signed but nobody uploaded, or a device
  that reports a lower counter than receipts already received, are visible
  only by comparing the two.

There is no partner bank yet, so no statement format to follow, and Mezgeb
is a generic ledger, so it must not be tied to one bank.

## Decision

1. **Three reconciliations, one store.** `recon_runs` records each run
   (kind `external`, `internal` or `offline`, its scope, counts and
   runtime), `recon_items` each statement line imported, with the ledger
   line it was matched to, and `recon_breaks` each disagreement found.
   Reconciliation never writes to the ledger tables. Its own rows are
   append-only, except that a break's status moves forward once.

2. **The ledger wins.** A break is a question about the other record first.
   A stale cache is reported with the ledger's balance and the client
   refreshes it; the ledger is never changed to match a cache. A statement
   break is resolved by posting an entry (decision 7), never by editing one.

3. **Statements are a generic CSV plus a mapping per bank.** Mezgeb defines
   one CSV layout (`docs/reconciliation/statement-format.md`): line ID,
   booking date, signed amount, currency and reference. A small TOML
   mapping per bank names that bank's columns, its date format, its sign
   convention (one signed column, or separate debit and credit columns) and
   the header lines to skip. ISO 20022 camt.053 can be added as a second
   reader later without changing anything else.

4. **Import is idempotent per line.** A statement line is identified by the
   reconciled account and the bank's line ID. The same line again, in a
   later import or repeated within one file, is the same bank transaction
   and is skipped; the same ID with different content is a
   `statement_conflict` break, and the stored line is never replaced.

5. **Matching never guesses.** Unmatched statement lines are matched with
   unmatched ledger lines of the same account, each line at most once:
   1. **Exact:** the entry's `bank_ref` metadata equals the statement line
      ID and the signed amounts are equal.
   2. **Rule:** same signed amount, same reference, booking date within the
      window (3 days by default), and exactly one candidate on each side.
      Two equal candidates are left unmatched, never picked between.

   A rule that cannot tell two lines apart leaves both as breaks: a break
   costs a person a look, a false match hides a loss. The target is zero
   false matches.

6. **Breaks are classified, and young items wait.** An item becomes a break
   only once it is older than the grace period (2 days by default): the
   client books a deposit after the statement shows it, so a fresh
   statement line without a ledger line is normal.

   | Kind | Reconciliation | Means |
   |---|---|---|
   | `missing_in_ledger` | external | The bank shows a movement the ledger does not |
   | `missing_in_statement` | external | The ledger shows a movement the bank does not |
   | `amount_mismatch` | external | Same `bank_ref`, different amount |
   | `duplicate_statement_line` | external | A statement line with its own ID repeats another's date, amount and reference, and has no ledger line of its own |
   | `statement_conflict` | external | A line ID imported again with different content |
   | `stale_cache` | internal | A client's cached balance differs from the ledger's at the same point |
   | `missing_receipts` | offline | A device reported counters the ledger never received receipts for, after their voucher closed |
   | `counter_behind` | offline | A device reported a lower counter than receipts already received |
   | `receipt_entry_mismatch` | offline | A settled receipt whose entry does not pay its amount |
   | `escrow_mismatch` | offline | A voucher's escrow balance differs from its amount less what it paid |

   One break is open per subject: a later run that finds the same
   disagreement does not raise it twice. A later run that matches what an
   open break is about (the bank's line arrives late, the client books the
   deposit) clears it.

7. **Money breaks are resolved through suspense templates only.** Whatever
   the cause, the first step is to make 1010 equal the bank again and park
   the difference in a suspense item, where ageing (step 2.02) keeps it
   visible until someone explains it:
   - the bank has **more** than the ledger: `deposit_unmatched.v1`,
     Dr 1010 / Cr `9010:sus-…` (E19, as today);
   - the bank has **less** than the ledger: the new
     `bank_debit_unmatched.v1`, Dr `9040:sbd-…` / Cr 1010, into the new
     control account **9040 Suspense: unexplained bank debits** (asset, one
     sub-account per item, returns to zero).

   Resolving a break records the entry that resolved it. The resolution is
   checked: the entry was booked by one of these two templates, it has not
   resolved another break, and its 1010 line equals the break's difference
   in amount and direction. That line is then matched to the break, so the
   next run does not report it again. The suspense item is closed later by
   `suspense_resolve.v1` (E20) or `bank_debit_resolve.v1`, Dr 1010 (the bank
   corrects it) or 5020 Bank charges / Cr `9040:sbd-…`, the whole balance.
   5020 becomes active for this.

8. **Other breaks are explained, not posted.** A `stale_cache` break closes
   when the client reports the refreshed balance or an operator marks it
   refreshed. Offline breaks point at the device and its receipts; the
   money side goes through the existing offline flows (step 2.13), and the
   break is closed with an operator's reason. `receipt_entry_mismatch` and
   `escrow_mismatch` mean the ledger disagrees with its own offline records:
   they are never closed without an explanation and are treated as a
   defect.

9. **Where it runs.** Statement import and the daily runs are
   `mezgeb-cli recon` commands, scheduled by the operator (the runbook,
   `docs/runbooks/reconciliation.md`). The API adds a cache reconciliation
   endpoint for clients (their own accounts only, up to 1,000 balances per
   call, each optionally as of an entry sequence so that postings in flight
   are not breaks), and endpoints to list breaks and resolve them. The
   device's reported last counter is stored with the voucher it asked for.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Never guess; unique candidates only (chosen)** | Zero false matches. Every break is real or ambiguous, never hidden. | Ambiguous days produce more breaks for a person to look at. |
| Best-score fuzzy matching | Fewer breaks to review. | A wrong match hides a real loss and is found only when the money is gone. |
| **Suspense entries first (chosen)** | 1010 equals the bank at once; the difference stays visible in ageing; the same two templates cover every cause. | Two postings where one correction would do. |
| Reverse or adjust the original entry | One posting. | Needs the cause before the books agree with the bank; an unexplained difference has no entry to reverse. |
| **Generic CSV plus mapping (chosen)** | Works with any bank today; keeps Mezgeb independent of one bank. | Each bank needs a mapping file; formats richer than CSV need their own reader. |
| ISO 20022 camt.053 only | A standard. | Not offered by every bank; nothing to test against yet. |
| Reconciliation as a separate service | Independent scaling. | Needs a second copy of the ledger data; the ledger already has the lines and their order. |

## Consequences

**Positive:**

- Every disagreement between the ledger and the bank, a client cache or a
  device is found and classified, with its age.
- Matching is deterministic: the same statement and ledger give the same
  matches, so a run can be repeated and compared.
- The ledger tables are untouched: reconciliation cannot break an
  invariant of the money.

**Negative:**

- Matching depends on clients recording the bank's line ID as `bank_ref`
  when they book a movement in 1010. Without it, only the rule pass
  matches, and busy days leave more breaks.
- A new account (9040) and two new templates, kept forever like every
  template version.

**Makes out of date:** the "Not decided yet" notes on bank debits in
[clearing and suspense](../accounting/clearing-and-suspense.md); 5020's
reserved status in the chart (ADR-0014).
