# 0003. Multi-line journal entries that must balance

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-10 |
| Answers | [Architecture §4](../architecture.md#4-data-model-at-a-glance), [§5.3](../architecture.md#53-guaranteeing-every-posted-operation-balances) |
| Implemented in | 1.04–1.06 (types), 1.08 (schema and constraint trigger), 1.09 (posting), 1.13 (verification) |

## Context

The scaffold copied a transfer model in which every transfer has exactly one
debit account and one credit account (spec INV-1). A merchant payment with a
fee then needs two transfers, linked so that both succeed or both fail.

The worked entries in step 1.01 show that many real events have three or
more lines: a payment with a fee (E07), a refund that returns a fee (E11),
an offline settlement (E17). Accountants read and review these as one
entry, and the trial balance is built from entries.

## Decision

The unit of posting is a **journal entry** with **two or more lines**. Each
line names one account, one side (debit or credit), a positive amount and a
currency. An entry posts only if, **for each currency, total debits equal
total credits**. Either every line posts or none does.

Balance is enforced at four independent levels:

1. **Types:** a `BalancedEntry` can only be built by a validator, so
   unbalanced data cannot reach the posting code.
2. **Engine:** checks again before writing.
3. **Database:** a `DEFERRABLE INITIALLY DEFERRED` constraint trigger
   rejects any commit where an entry's debits and credits differ per
   currency.
4. **Verification:** a scheduled job checks that global debits equal global
   credits and that every account's totals equal the sum of its lines. A
   failure alerts someone.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Multi-line entries (chosen)** | Matches accounting practice and the worked entries one to one. One entry per business event. The trial balance is read directly from entries. | Validation is more complex (per-currency sums, minimum two lines). More rows per entry than a single transfer. |
| One debit, one credit per transfer, linked chains | Simple fixed-width records, very fast in specialised engines. | A fee payment is two records joined by a link flag. Atomicity depends on the chain. Hard for accountants to review. |
| Single-entry balance updates | Simplest to build. | No proof that money is neither created nor destroyed. Not acceptable for a system of record. |

## Consequences

**Positive:**

- A fee can never be charged for a payment that failed, because the payment
  and the fee are one entry (E07).
- An accountant can review postings in the same form as the worked entries.

**Negative:**

- The scaffold's transfer types in `mezgeb-core` must be replaced
  (steps 1.04–1.06).
- Rules that relied on exactly two accounts, such as "debit account ≠ credit
  account", must be restated for lines.

**Makes out of date:** spec §2 INV-1 and INV-6, §3.5 (transfer), §4.4
(linked transfers) and §4.5 (balancing transfers). They are rewritten in
steps 1.03 and 1.06.
