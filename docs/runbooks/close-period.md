# Runbook: Closing an Accounting Period

[Runbooks](README.md) · Code: [`crates/engine/src/close.rs`](../../crates/engine/src/close.rs) · Step: [2.05](../roadmap/phase-2-operations-and-security/05-accounting-periods-and-closing.md) · Decision: [ADR-0016](../adr/0016-accounting-periods.md)

Closing a period freezes its balances. Reports issued for a closed period
never change afterwards, because nothing can be posted into it again
(INV-10). Close the operational period **once a day**, shortly after
midnight. A monthly accounting close is the last daily close of the month,
followed by the checks in [After a month-end close](#after-a-month-end-close).

| | |
|---|---|
| Who | The on-call operator |
| When | Daily, after 00:00 (local business day) |
| Takes | Seconds for a day's postings; postings pause only for the switch |
| Command | `mezgeb-cli close-period --next <label>` |
| Connects as | `mezgeb_app` (it writes the periods and their snapshots) |
| Safe to repeat | Yes |

## What the close does

1. **Switch.** It waits for postings already under way to commit (they
   hold the period lock shared; the switch takes it exclusively for an
   instant), marks the open period `closing`, records its last entry, and
   opens the next period. Every new posting now goes to the next period.
   New postings wait while it switches, for as long as the slowest
   posting already under way needs to commit. Usually that is
   milliseconds; a posting that would wait longer than its lock timeout
   (5 seconds by default) is refused with `Busy`, and the client retries
   it.
2. **Verify.** For every entry of the period, it checks that it lies in the
   period's range and balances in every currency. For every account, it
   computes the totals at the end of the period twice, from the previous
   snapshot plus the period's lines and from the stored running totals, and
   checks that they agree.
3. **Snapshot and close.** It writes each account's totals as the period's
   snapshot and marks the period `closed`.

If step 2 finds a problem, nothing is written: the period stays `closing`,
postings continue in the next period, and the command exits with status 1.

## Before the close

1. **Choose the label of the period being opened.** Use the business date
   it covers, `YYYY-MM-DD` (for example `2026-11-15`). A label is used once;
   a label that already exists is refused and nothing changes.
2. **Check that no period is stuck in `closing`** from an earlier run:

   ```sql
   SELECT id, label, status, closing_at FROM periods WHERE status <> 'closed' ORDER BY id;
   ```

   Exactly one `open` row is normal. A `closing` row means an earlier close
   stopped part-way or found problems; see [When the close fails](#when-the-close-fails).

## Run the close

```sh
MEZGEB_DATABASE_URL=postgres://mezgeb_app:…@<host>/mezgeb \
    mezgeb-cli close-period --next 2026-11-15
```

A clean close prints, and exits 0:

```text
period 41 (2026-11-14): closed, entries 812004..=845913, 1203 accounts snapshotted
postings now go to period 42 (2026-11-15)
```

## After the close

1. **Run verification** as `mezgeb_verifier`, which checks every account
   against the whole journal, not only the closed period:

   ```sh
   mezgeb-cli verify --database-url postgres://mezgeb_verifier:…@<host>/mezgeb
   ```

2. **Issue the day's trial balance from the snapshot**, which will not
   change (`trial_balance(AsOf::Period(<id>))`). Check that it balances in
   every currency.
3. **Record the close** in the operations log: period id, label, entry
   range, accounts snapshotted, and the verification run id.

## When the close fails

| Exit status | Meaning | What to do |
|---|---|---|
| 2, "no accounting period is open" | No period is open: postings are being refused | Page the ledger owner. Do not create a period by hand. |
| 2, "duplicate key … periods_label_key" | The label is used | Run again with the right label. Nothing changed. |
| 2, a connection or database error | The close could not run, or stopped part-way | Run the same command again. If the switch had already happened, the command finishes that period and opens nothing new. |
| 1, findings printed | The period's journal is inconsistent | Leave the period `closing`; postings are unaffected. Run `mezgeb-cli verify --full`, open an incident with the findings, and do not issue reports for the period. Once the cause is corrected (by correcting entries in the open period, never by editing history), run the command again: it retries the same period. |

A period that stays `closing` blocks the next close until it is closed,
because only one period may be closing at a time. The open period keeps
taking postings meanwhile.

## After a month-end close

A monthly accounting period is the run of daily periods in that month. After
the last daily close of the month:

1. Produce the month's trial balance from that last period's snapshot.
2. Reconcile the customer funds account (1010) against the bank statement
   at the same cut-off (step 2.15 automates this).
3. File the trial balance and the verification run id with the month's
   records.

## Late postings

An entry is always posted into the period that is open when it is posted,
never into a closed one. A late event, such as an offline payment from last
week, posts into today's period and keeps its original date in
`event_time`. Reports by period show it on the day it was posted; reports
by event date read `event_time`.

## Never

- Never update or delete rows in `periods` or `period_snapshots` by hand.
  The database refuses it (`MZ008`, append-only triggers).
- Never post a correction into a closed period. Corrections are new entries
  in the open period (reversals, adjustments).
- Never run two closes at once. The second waits for the first; it is safe,
  but pointless.
