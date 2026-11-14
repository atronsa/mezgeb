# 0016. Accounting periods follow posting order; a close switches periods under a lock

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-14 |
| Answers | Spec §6 (periods and daily close); INV-10 |
| Implemented in | 2.05 (accounting periods, closing and trial balance) |

## Context

Reports for a period must never change once issued, so nothing may be
posted into a closed period (INV-10). But postings never stop: a period has
to close while other postings are being written, and an entry that started
before the close must not land in the period after it has been verified and
snapshotted. Some events also arrive late (offline payments), with an
original date in an earlier period.

## Decision

1. **An entry belongs to the period that is open when it is posted**
   (`journal_entries.period_id`). Its original date is its `event_time`; a
   late event posts into today's period. There is exactly one open period.
2. **Periods are contiguous ranges of posting order.** A period holds the
   entries with `after_sequence < sequence <= last_sequence`, so a snapshot
   is simply every line up to `last_sequence`.
3. **The switch is an advisory lock.** Every posting takes
   `pg_advisory_xact_lock_shared` before it reads the open period and holds
   it until it commits. Closing takes the same lock exclusively for an
   instant: it waits for postings under way, records the period's
   `last_sequence`, marks it `closing` and opens the next. No posting can be
   between reading the period and committing while the switch happens.
4. **Verify, snapshot, close.** The closing period is verified (range,
   balance per entry, and totals from snapshot plus lines against stored
   running totals), then its snapshots are written and it is marked
   `closed`. With findings, it stays `closing` and nothing is written.
5. **The database enforces it again.** An entry or line may only be written
   into an open period, checked on insert and for the entry again at commit;
   periods only move forward; snapshots are append-only (`MZ008`).

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Period by posting, switched under an advisory lock (chosen)** | Exact: a period is a sequence range; no wait on time | Every posting takes one more (shared, uncontended) lock; postings pause during the switch |
| Period by effective date chosen by the client | Reports by business date | A client could post into any open past period; closing would have to refuse late events or keep periods open indefinitely |
| Period by `recorded_time` with a settle delay (as verification does) | No lock | Recorded time is not commit order (INV-12); a slow transaction could still commit into a verified range, so the close must wait out the worst case |
| Row lock on the period row (`FOR SHARE`) | Ordinary locking | Needs UPDATE privilege for the app role on the period row, and every posting would write a lock to it |

## Consequences

**Positive:**

- Snapshots are exact, and "snapshot + later lines = current balance"
  holds by construction.
- The close needs no settle time.

**Negative:**

- Postings pause for as long as the slowest in-flight posting needs to
  commit, once per close; one that would wait past its lock timeout is
  refused with `Busy` and retried by the client.
- A period that cannot be verified stays `closing` and blocks the next
  close until it is resolved.
