# 0011. History is append-only; corrections are new entries

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-10 |
| Answers | [Architecture §5.6](../architecture.md#56-reversals-without-modifying-history) |
| Implemented in | 1.08 (triggers and role privileges), 1.12 (reversals), 2.04 (refunds and adjustments), 2.09 (audit) |

## Context

Mistakes happen: a deposit is credited to the wrong wallet (E13), a merchant
refunds a sale (E11), support compensates a customer (E14). If entries could
be edited or deleted, a mistake, or a theft, could be hidden, and no balance
in the past could be trusted.

## Decision

Posted entries and lines are **never updated or deleted**. Every correction
is a **new entry** that references what it corrects:

| | Reversal | Refund | Adjustment |
|---|---|---|---|
| For | A booking mistake | A business decision to give money back | A manual correction or compensation |
| Lines | An exact mirror of the original | A new operation, may be partial and may treat fees differently | Against an expense or suspense account |
| Link | `reverses_entry_id`, unique, so an entry is reversed at most once | References the original payment | A reason code |
| Approval | Operations | The merchant | Maker-checker: a second person approves |

"Is reversed" is never a column on the original entry. It is derived from
the existence of the reversal.

`UPDATE` and `DELETE` on journal tables are blocked **twice**: by triggers,
and by the ledger's database role, which has no such privilege.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Append-only, corrections as new entries (chosen)** | Full audit trail. Past balances are reproducible. Matches accounting practice. | More entries. Reports must show reversed pairs clearly. |
| Edit the original entry, with an audit log of changes | Fewer entries. The "current" view is simple. | The journal and the audit log can disagree. Past reports change silently. |
| Soft delete (a `deleted` flag) | Easy to undo. | Balances depend on a flag that anyone with update rights can flip. |

## Consequences

**Positive:**

- Replaying the journal from empty reproduces every balance (spec INV-8).
- Combined with hash chaining (spec §7.1), tampering becomes detectable.

**Negative:**

- A mistake stays visible forever. This is intended.
- Fixing a mistake takes two entries (reversal and correct entry), as in
  E13.
