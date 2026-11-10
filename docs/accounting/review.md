# Accounting Review

[Accounting model](README.md) · Reviews: [chart of accounts](chart-of-accounts.md), [worked entries](entries/README.md), [trial balance](trial-balance.md)

> **Status:** Waiting for reviewer.

Mezgeb is an open-source double-entry ledger for wallet and payment
platforms. Before any code is written, every common money flow is booked by
hand as a journal entry. This review asks someone with accounting knowledge
to confirm that those entries are correct, because the engine will be built
to post exactly what they say.

## What to review

About an hour of reading, in this order:

1. [Accounting model](README.md): conventions and normal balances (2 minutes).
2. [Worked entries](entries/README.md): the opening position and E01–E18,
   one continuous story about two customers and one merchant. Each entry
   has a scenario, the journal lines, the reasoning and open questions.
3. [Chart of accounts](chart-of-accounts.md): the accounts the entries use.
4. [Trial balance](trial-balance.md): the closing balances and checks.

All entries are booked from the **platform's** point of view: money it holds
for customers and merchants is a liability, not its own money.

## Checklist for the reviewer

- [ ] Each entry debits and credits the right accounts, on the right sides.
- [ ] Each account has the right type (asset, liability, equity, revenue,
      expense) and normal side.
- [ ] Customer and merchant money is kept apart from the platform's own money
      (bank accounts 1010 and 1020).
- [ ] Refunds, reversals and adjustments (E11–E14) are treated correctly and
      are not confused with one another.
- [ ] Withdrawals that fail (E04) and withdrawals the bank returns (E05) are
      treated correctly.
- [ ] Pending and escrow balances (2020, 2040, 2050) are correctly shown as
      liabilities.
- [ ] The trial balance follows from the entries, and its checks are the
      right ones.
- [ ] Nothing important is missing: a common flow with no entry, or an
      account that should exist.

## Questions for the reviewer

These need accounting judgment. Product questions (limits, fees, retry
rules) are left in the entries and are not part of this review.

| # | Question | Where |
|---|---|---|
| Q1 | Pending withdrawals, pending settlements and offline escrow are booked as separate liability accounts rather than as flags on the wallet. Is that a sound treatment? | E02, E15, E16 |
| Q2 | Fees are recognised as revenue at payment time, while the cash stays in the customer funds account until swept. Is that acceptable, and how often should fees be swept to the operating account? | E07, chart: safeguarding check |
| Q3 | Refunded fees are debited straight to fee revenue (4010). Should they go to a contra-revenue account so gross fee income stays visible? | E11, E12 |
| Q4 | If refunds exceed a merchant's balance, should the merchant sub-account go negative, or should the debt move to an asset account (receivables from merchants)? | E11, E15 |
| Q5 | A goodwill credit is an expense (5010), funded by a transfer from the operating account. Should the expense and the funding be one entry, so customer money is never short? The trial balance shows a 7.80 shortfall between them. | E14, trial balance |
| Q6 | Should confirmed but unsettled deposits pass through a clearing account (9020), and unmatched money through a suspense account (9010), before reaching a wallet? Are their types right (asset and liability)? | E01, E05, E13 |
| Q7 | An offline payment is booked when the receipt syncs, but carries the time it actually happened. If the two fall in different periods, which period does the payment belong to? | E17 |
| Q8 | If a receipt arrives after its escrow was returned, the loss goes to an expense account (5030). Is that the right treatment, or should it be a receivable from the customer first? | E18, chart: 5030 |
| Q9 | Which fee rounding rule is acceptable (round half up, in the merchant's favour, banker's rounding)? | E07, E12 |

## Findings

The reviewer records anything wrong or unclear here. Each finding is fixed
in the entries, chart or trial balance, then marked resolved.

| # | File or entry | Finding | Severity | Resolution | Status |
|---|---|---|---|---|---|
| | | | | | |

**Severity:** *Error*: the entry is wrong and must be fixed before step 1.01
closes. *Change*: correct, but a better treatment exists. *Note*: a
question or clarification, with no change needed.

## Sign-off

Filled in by the reviewer when every *Error* finding is resolved.

| | |
|---|---|
| Reviewer | |
| Background | |
| Date | |
| Scope | Chart of accounts v0, worked entries OB and E01–E18, trial balance after E18 |
| Answers to Q1–Q9 | |

> I have reviewed the worked entries, chart of accounts and trial balance
> listed above. The entries are correctly booked from the platform's point of
> view, subject to the resolved findings recorded here.

After sign-off, the reviewer's name and date are copied into the
[accounting model](README.md), and the matching exit item is ticked.
