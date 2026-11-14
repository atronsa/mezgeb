# 0015. Refunds name the entry they refund and are capped by it; disputes hold merchant funds in their own sub-account

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-14 |
| Answers | E11's and E12's open questions ([refunds](../accounting/entries/refunds-and-corrections.md)); refund vs reversal |
| Implemented in | 2.04 (refunds and disputes) |

## Context

A refund gives money back for a payment that really happened. Unlike a
reversal (ADR-0011, INV-14), it may be partial, there may be several, and
the money may come from somewhere other than the original accounts (a
dispute's held funds, for example). Refunding more than was paid, refunding
twice, or refunding to someone else are each a direct theft path, and
concurrent refunds of one payment must not slip past a limit together.

The engine must enforce this without knowing anything about payments,
merchants or templates (ADR-0002).

## Decision

**1. A refund is an entry of its own kind that names the entry it refunds**
(`EntryDraft::refunds`, entry kind `refund`, column `refunds_entry_id`).
Only a standard entry can be refunded.

**2. The limit is derived from the original entry itself, per account.** An
account the original debited may only be credited, an account it credited
may only be debited, each by at most what the original moved on it across
all refunds together; an account the original did not touch may only be
debited (INV-15). The client never states the limit, so it cannot lie
about it. The rule is generic: it needs no knowledge of which account is
the payer or the merchant.

**3. The engine enforces it with a per-entry tally, locked.** Each refunded
entry has one `refund_tallies` row per account, created on first use. A
refund locks the rows, checks, posts and updates them in one transaction,
so concurrent refunds of one entry queue. The tally table refuses to exceed
its limit or shrink; a deferred trigger checks the rule again from the
journal at commit.

**4. Refunds and reversals exclude each other.** A reversed entry cannot be
refunded, an entry with refunds cannot be reversed, and a refund is never
reversed. A reversal locks the same tally rows, so a refund and a reversal
of one entry never both succeed. Any other correction is an adjustment
(ADR-0013).

**5. Disputes hold merchant funds in their own sub-account** (2060, one per
dispute), not in a ledger hold, because a dispute must never be decided by
a timer. A decision for the payer is a refund of the disputed payment, so
it shares the payment's refund limit.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Per-account limits derived from the original, with locked tallies (chosen)** | Generic; covers the payer, the merchant and the fee at once; nothing for a client to get wrong | One more table, and a refund locks one row per account of the original |
| A single "refundable amount" stated by the client | Simple | The client could state any limit; nothing stops refunding the fee twice |
| Refunds as partial reversals | Reuses INV-14 | A reversal is an exact mirror by definition; partial mirrors break the database's mirror check |
| Compute the limit at refund time from all earlier refunds, without a tally | No extra table | Two concurrent refunds both see the same past and can both pass; locking the journal is not possible for the app role |
| Disputes as ledger holds on the merchant | Reuses step 1.14 | A hold expires by itself, and an expiry would decide the dispute |

## Consequences

**Positive:**

- Over-refunds, double refunds and refunds to someone else are impossible,
  whatever the client sends, and under any concurrency.
- A dispute decided for the payer cannot be followed by a refund of the
  same money.

**Negative:**

- A refunded entry can no longer be reversed; a mistake in it is corrected
  with an adjustment.
- A refund takes row locks on its original's tallies, so refunds of one
  payment queue (refunds of different payments do not).

**Makes out of date:** E11's and E12's open questions on fees and limits.
