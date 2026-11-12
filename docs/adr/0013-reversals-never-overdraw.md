# 0013. A reversal that would overdraw an account is refused; corrections then use an adjustment

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-12 |
| Answers | E13's open question ([worked entries](../accounting/entries/refunds-and-corrections.md#e13-reversal)) |
| Implemented in | 1.12 (reversals and adjustments) |

## Context

A reversal is the exact mirror of an entry posted by mistake (ADR-0011,
INV-14). In E13, a 500 ETB deposit is credited to the wrong wallet and
reversed the next day. But if the customer has already spent some of the
money, the mirror would take the wallet below zero, which INV-8 forbids for
customer wallets.

The ledger needs a rule for this case that is the same every time, and that
never creates or hides money.

## Decision

A reversal is a normal posting and passes every posting rule, including
INV-8. **If it would take any account below zero on its normal side (for an
account that does not allow it), it is refused** with insufficient funds,
and nothing changes.

The mistake is then corrected by an **adjustment**: a new entry with an
adjustment reason (for example `write_off` or `compensation`), requested
by one person and approved by another (INV-25), for the amount that can
actually be corrected. Recovering the rest from the customer is a business
decision outside the ledger.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Refuse, then correct with an approved adjustment (chosen)** | No account ever breaks INV-8. Every correction beyond a clean mirror needs two people and a reason. One rule for every reversal. | The original entry stays unreversed; reading the history needs the adjustment that corrected it. |
| Allow reversals to overdraw (a "reversal exception") | The mistake is undone in one step. | Creates a negative customer balance, a debt the ledger has no account for. Breaks INV-8 for one kind of entry, so every report must special-case it. |
| Partial reversal (mirror only what is left) | Undoes as much as possible automatically. | No longer an exact mirror, so INV-14 and the database's mirror check no longer hold; a partial reversal is an adjustment by another name, without the second approver. |
| Queue the reversal until the money comes back | Eventually exact. | The books stay wrong for an unknown time, and an operation waits on a customer's future deposits. |

## Consequences

**Positive:**

- INV-8 has no exceptions.
- Any correction that is not a clean mirror goes through maker-checker.

**Negative:**

- Operations need a procedure for a refused reversal: propose an
  adjustment, get it approved, and record the recovery decision.

**Makes out of date:** the E13 open question, now answered by this ADR.
