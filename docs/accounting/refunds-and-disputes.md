# Refunds and Disputes

[Accounting model](README.md) · [Chart of accounts](chart-of-accounts.md) · Decided in step [2.04](../roadmap/phase-2-operations-and-security/04-refunds-and-disputes.md) · [ADR-0015](../adr/0015-refunds-and-disputes.md)

> **Status:** Draft for review. **Reviewer:** _TBD (product owner and someone with accounting knowledge)_ · **Reviewed on:** _TBD_
>
> The step's exit gate needs this policy written down and reviewed. It is
> written down; the review is pending.

Giving money back is the most direct way to steal from a payment system:
refund more than was paid, refund the same payment twice, or refund to
someone who never paid. This page says how Mezgeb books refunds and
disputes so that none of these is possible, and what the business decides
around them.

## Refunds

A refund is a new entry that **names the payment it refunds**. The original
stays in history, unchanged and still correct (ADR-0011). A refund is a
business event, decided by the merchant; a **reversal** (E13) is for a
booking mistake, and the two never apply to the same payment (ADR-0015).

**The refund rule (INV-15).** For every account of the original payment:

| The original… | A refund may… | At most, over all refunds |
|---|---|---|
| debited it (the payer) | only credit it | what the original debited |
| credited it (the merchant, fee revenue) | only debit it | what the original credited |
| did not touch it | only debit it | (no limit from the payment; the posting rules still apply) |

So money goes back only to whoever paid, and never more than they paid,
however the refund is split and however many refunds there are.

**Enforced three times.** The engine checks the rule inside the refund's own
transaction, with the payment's **refund tallies** locked, so concurrent
refunds of one payment queue and the second sees the first. The tallies
table itself refuses to go above the payment or to shrink. At commit, the
database checks the rule again from the journal alone (migration 0006).

| Rule | Error |
|---|---|
| A refund would give back more than the payment moved on some account | `RefundExceedsOriginal` |
| A refund line credits someone the payment did not debit, or moves an account the wrong way | `RefundLineNotAllowed` |
| The payment has been reversed | `RefundOfReversedEntry` |
| The entry is a reversal, an adjustment or a refund | `NotRefundable` |
| A payment with refunds is reversed | `RefundedEntryCannotBeReversed` |
| A refund is reversed | `CannotReverseRefund` (a mistaken refund is corrected with an adjustment) |

**The fee on a refund.** Whether the platform gives back its fee is a
business decision, made by the client for each refund: `merchant_refund.v1` takes
`fee_refund`, the part of the refund that comes out of fee revenue. The rest
comes out of the merchant's balance. The ledger never computes it, but it
never lets the fee refunds of a payment exceed its fee.

| Policy | `fee_refund` | Example |
|---|---|---|
| Fee returned in proportion (the default in the worked entries) | the fee × refund ÷ price, rounded by the client's rule | E11 (all of it), E12 (0.80 of 2.00) |
| Fee kept by the platform | absent | the merchant pays back the whole refund |

Fee refunds reduce 4010 directly, as in E11 and E12. A separate
contra-revenue account is not used.

**When the merchant's balance is too low** (for example, after a
settlement payout, E15), the refund is refused by the posting rules: a
merchant balance never goes below zero. The merchant must be funded first,
or the platform pays the refund from its own money with an approved
adjustment.

## Disputes

A dispute is a payer's claim that a payment should not stand. The lifecycle:

```
opened ──► held (2060:dsp-…) ──resolved for the payer────► payer's wallet  (a refund of the payment)
                              └─resolved for the merchant─► merchant's balance
```

| Event | Template | Lines | Entry |
|---|---|---|---|
| The payer opens a dispute | `dispute_open.v1` (requires `disputed_entry`) | Dr `2030:merch-…` / Cr `2060:dsp-…` (new) | E21, E23 |
| Decided for the payer | `dispute_resolve_payer.v1`, a refund of the disputed payment | Dr `2060:dsp-…` / Cr `2010:cust-…`, the whole item | E22 |
| Decided for the merchant | `dispute_resolve_merchant.v1` | Dr `2060:dsp-…` / Cr `2030:merch-…`, the whole item | E24 |

**The rules:**

1. **The disputed money is held in its own sub-account.** Each dispute has
   one `2060:dsp-…` sub-account. While it is open, neither the merchant nor
   the payer can spend the money, and it still counts as money owed in the
   safeguarding check. It is not a ledger hold: a hold expires by itself,
   and a dispute must never be decided by a timer.
2. **At most what the merchant received is held.** The platform's fee is not
   held. If the payer wins, the platform keeps its fee, unless the client
   also books a refund of it.
3. **A dispute is decided wholly for one side.** Both resolutions move the
   whole item, so a dispute has exactly one outcome; a second decision is
   refused by the template and by the posting rules (the item is at zero).
4. **A decision for the payer is a refund of the payment.** It counts
   against the payment's refund limit like any refund, so a payer can never
   recover the same money twice, once by refund and once by dispute. If the
   payment was reversed, the decision for the payer is refused; the
   dispute is then decided for the merchant, because the reversal already
   gave the payer their money back.
5. **When the merchant's balance cannot cover the dispute, opening it is
   refused.** Mezgeb does not hold part and put the rest in suspense: a
   dispute item always holds real merchant money. The client keeps the case
   open and tries again when the merchant has funds (for example, before
   the next settlement), or the platform decides to fund the payer from its
   own money with an approved adjustment (`compensation`), and recovers it
   from the merchant later.
6. **Open disputes are aged.** 2060 is in the ageing report
   (`mezgeb_ops::AGED_ACCOUNTS`). The time limit for deciding a dispute is a
   business rule, set by the operator.

## For the reviewer

Please confirm or change:

- the default fee policy (proportional fee refund, kept on a dispute the
  payer wins);
- refusing to open a dispute the merchant's balance cannot cover, instead of
  holding part of it;
- that a refunded payment can no longer be reversed, and a refund is never
  reversed (corrections then go through adjustments);
- whether open disputes should block a merchant's settlement payout
  (not done yet; step 3.07).
