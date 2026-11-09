# Refunds and Corrections (E11–E14)

[All entries](README.md) · [Chart of accounts](../chart-of-accounts.md)

Three different ways of moving money back, which are easy to confuse:

| | Refund (E11, E12) | Reversal (E13) | Adjustment (E14) |
|---|---|---|---|
| Why it happens | A business decision: the merchant gives money back | A booking mistake: an entry should never have been posted | A manual correction or compensation with no original entry to undo |
| Who decides | The merchant | Operations, after finding the mistake | Operations, with a second approver |
| Lines | New entry, may differ from the original (e.g. partial) | Exact mirror of the original entry | New entry against an expense or suspense account |
| Original entry | Stays, and is still correct | Stays, marked as reversed | Not applicable |

In all three, **history is never edited**. Every correction is a new entry
that references what it corrects.

The examples continue from E10: Abebe's wallet holds 470 ETB, Hana's 180 ETB,
and Merkato Coffee's balance is 344 ETB.

## E11. Full refund

**Scenario:** The coffee machine Abebe paid for by card in E08 (200 ETB) is
faulty. Merkato Coffee refunds the full amount. The platform's policy for full
refunds is to give back its 1.5% fee too, so the merchant is not charged for a
sale that did not happen.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2030:merch-coffee Merchant balance | Liability | 197 | |
| 2 | 4010 Payment fee revenue | Revenue | 3 | |
| 3 | 2010:cust-abebe Customer wallet | Liability | | 200 |
| | **Total** | | **200** | **200** |

**Why:**

- **Line 1, debit the merchant balance (liability ↓).** The merchant gives up
  the 197 ETB it received in E08, so the platform owes Merkato Coffee less.
- **Line 2, debit fee revenue (revenue ↓).** The platform gives up the 3 ETB
  fee it earned in E08. Revenue decreases on the debit side.
- **Line 3, credit Abebe's wallet (liability ↑).** Abebe gets the full 200 ETB
  back.
- **The equation stays in balance:** Liabilities −197 + 200, Revenue −3. The
  net effect undoes E08, but as a new entry on the day of the refund, which
  references E08.

**Why not a reversal?** E08 was correct: the payment really happened. The
refund is a separate business event on a later date, and it may differ from
the original (see E12). A reversal is only for entries booked by mistake (E13).

**Open questions:**

- Who keeps the fee on a refund is a business policy, not an accounting rule.
  Should the platform keep it (the merchant pays 200 and Abebe gets 200), or
  return it, as here? Many card networks keep it.
- What if the merchant's balance is lower than the refund because it was
  already paid out (E15)? Does the refund fail, does the merchant's balance go
  negative, or does the merchant need to fund it first?
- Should the fee be reversed into a separate contra-revenue account ("fee
  refunds") so gross fee income stays visible in reports?

## E12. Partial refund

**Scenario:** One of the two items Abebe bought with the QR payment in E07
(100 ETB, 2% fee) was missing. Merkato Coffee refunds 40 ETB. The platform
returns the fee in proportion: 2% of 40 ETB = 0.80 ETB.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2030:merch-coffee Merchant balance | Liability | 39.20 | |
| 2 | 4010 Payment fee revenue | Revenue | 0.80 | |
| 3 | 2010:cust-abebe Customer wallet | Liability | | 40.00 |
| | **Total** | | **40.00** | **40.00** |

**Why:**

- The lines are the same as in E11, for part of the amount: the merchant gives
  back 39.20, the platform gives back 0.80 of fee, and Abebe receives 40.
- **This is why money is stored in minor units.** In santim, the entry is
  3,920 + 80 = 4,000, all whole numbers. With floating-point numbers, 0.8 and
  39.2 cannot be stored exactly, and over millions of entries the totals
  would drift.
- **The refund is limited by the original.** The total refunded against E07
  can never exceed 100 ETB. The ledger checks this, not only the client
  application.

**Open questions:**

- When the proportional fee does not divide evenly into santim (for example,
  2% of 33.33 ETB), who gets the extra santim, the merchant or the platform?
  This must match the rounding rule chosen in E07.
- Should several partial refunds against one payment be tracked so that their
  fee refunds add up exactly to the original fee?

## E13. Reversal

**Scenario:** A bank deposit of 500 ETB arrives with a customer reference that
the matching job reads wrongly. It is credited to Abebe's wallet, but it was
sent by Hana. Operations notices the next day, before Abebe has spent it.

**(a) The mistaken entry** (booked like E01, but to the wrong wallet):

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 1010 Bank: customer funds account | Asset | 500 | |
| 2 | 2010:cust-abebe Customer wallet | Liability | | 500 |
| | **Total** | | **500** | **500** |

**(b) E13, the reversal:** an exact mirror of (a), with the debits and credits
swapped:

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-abebe Customer wallet | Liability | 500 | |
| 2 | 1010 Bank: customer funds account | Asset | | 500 |
| | **Total** | | **500** | **500** |

**(c) The correct entry:** a normal deposit (E01) to Hana's wallet:

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 1010 Bank: customer funds account | Asset | 500 | |
| 2 | 2010:cust-hana Customer wallet | Liability | | 500 |
| | **Total** | | **500** | **500** |

**Why:**

- **Entry (a) is not deleted or edited.** It really was posted, and Abebe may
  even have seen the balance. Deleting it would hide what happened and break
  the audit trail. Instead, (b) cancels its effect, and both stay in history.
- **The reversal is an exact mirror.** Same accounts, same amounts, opposite
  sides. Added together, (a) and (b) leave every balance exactly as it was.
  The reversal references (a), and (a) can be reversed only once.
- **The correction is a separate entry.** (c) books what should have happened.
  Keeping (b) and (c) separate makes the history read clearly: "booked to the
  wrong wallet, cancelled, booked to the right wallet."
- **Net effect of (a) + (b) + (c):** the bank has 500 ETB more, and Hana's
  wallet has 500 ETB more. Abebe's wallet is back where it started.

**Open questions:**

- What if Abebe had already spent some of the 500 ETB? The reversal would make
  the wallet negative. Is that allowed for reversals only (the debt is then
  collected), or must the reversal wait for manual handling?
- Should deposits with uncertain references go to a suspense account first
  (see E01), so that mistakes like (a) are caught before reaching a wallet?
- Who can trigger a reversal, and does it need a second approver?

## E14. Admin adjustment

**Scenario:** Abebe complains that a failed withdrawal (E04) took three days
to resolve. Customer support offers a goodwill credit of 10 ETB. A support
agent proposes the adjustment, and a supervisor approves it (maker-checker).
The platform pays for it from its own money, not from other customers' funds.

**(a) The adjustment:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 5010 Customer compensation expense | Expense | 10 | |
| 2 | 2010:cust-abebe Customer wallet | Liability | | 10 |
| | **Total** | | **10** | **10** |

**(b) Funding the customer funds account:** the platform moves 10 ETB from
its own operating bank account into the customer funds account:

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 1010 Bank: customer funds account | Asset | 10 | |
| 2 | 1020 Bank: operating account | Asset | | 10 |
| | **Total** | | **10** | **10** |

**Why:**

- **(a) line 1, debit an expense (expense ↑).** The platform is giving away
  10 ETB of its own money. That is a cost of doing business, and expenses
  increase on the debit side.
- **(a) line 2, credit Abebe's wallet (liability ↑).** The platform now owes
  Abebe 10 ETB more.
- **Why (b) is needed.** After (a), the platform owes customers 10 ETB more,
  but no new cash has arrived in the customer funds account. Without (b),
  the customer funds account would hold less than the platform owes
  customers, and customers' money would be paying for the platform's
  goodwill. Moving 10 ETB from the operating account (1020) restores the
  backing.
- **(b) moves money between two assets.** One bank account goes up, the other
  goes down. Total assets do not change.

**Every adjustment has an approver and a reason.** An adjustment creates
money in a customer's wallet without a payment behind it, so it is the
easiest way to steal from a ledger. Mezgeb requires a second person to approve
every adjustment, a written reason, and an audit event (step 2.08).

**Open questions:**

- Should (a) and (b) be one four-line entry, so the wallet can never be
  credited without the funding? Or is (b) a separate daily top-up that covers
  all adjustments of the day?
- Is there a limit on how much one agent can propose, or one supervisor can
  approve, per day?
- Which expense accounts are allowed for adjustments (compensation, fraud
  losses, bank charges), and who maintains that list?
