# Deposits and Withdrawals (E01–E05)

[All entries](README.md) · [Chart of accounts](../chart-of-accounts.md)

## E01. Deposit via bank

**Scenario:** Abebe transfers 1,000 ETB from a personal bank account to the
platform's customer funds account at the partner bank. The bank confirms the
funds have arrived, and the platform credits Abebe's wallet. There is no fee.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 1010 Bank: customer funds account | Asset | 1,000 | |
| 2 | 2010:cust-abebe Customer wallet | Liability | | 1,000 |
| | **Total** | | **1,000** | **1,000** |

**Why:**

- **Line 1, debit the bank account (asset ↑).** The platform now has 1,000 ETB
  more cash in its bank account. Cash is something the platform *owns*, so it
  is an asset, and assets increase on the debit side.
- **Line 2, credit Abebe's wallet (liability ↑).** The money is not the
  platform's: Abebe can withdraw it at any time. The platform now *owes* Abebe
  1,000 ETB, so that wallet balance is a liability, and liabilities increase on
  the credit side.
- **The equation stays in balance:** Assets +1,000 = Liabilities +1,000. Equity
  does not change, because the platform has earned nothing. A deposit is never
  revenue.

**The common confusion:** Abebe's own bank statement shows the payment as a
*debit* to that account, and the wallet app shows a balance that went *up*. Both
are true from other points of view. These books are the **platform's**, and
from the platform's point of view a customer's balance is a debt it owes.
That is why every customer wallet in Mezgeb is a liability with a credit
normal balance.

**Open questions:**

- Bank notifications often arrive before the funds are final. Should a
  confirmed-but-unsettled deposit go through a clearing account (9xxx) first,
  and only reach the wallet when the bank settles? This decides whether E01
  becomes two entries.
- What happens when a deposit arrives with a missing or wrong customer
  reference? The money is real but has no owner yet, so it probably belongs in
  a suspense account until it is matched.
- If the bank charges a fee for incoming transfers, who pays it, the customer
  or the platform? If the platform pays, it is an expense (5xxx) booked in a
  separate line.

## E02. Withdrawal: requested

**Scenario:** After E01, Abebe's wallet holds 1,000 ETB. Abebe asks to
withdraw 300 ETB to a personal bank account. The platform accepts the request
but has not sent the bank transfer yet; payouts go out in a batch later in the
day (E03). There is no fee.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-abebe Customer wallet | Liability | 300 | |
| 2 | 2020:wd-0001 Pending withdrawals | Liability | | 300 |
| | **Total** | | **300** | **300** |

**Why:**

- **No cash has moved yet, so the bank account (1010) is not touched.** The
  1,000 ETB is still sitting in the platform's bank account. Booking cash out
  now would make the books disagree with the bank statement.
- **Line 1, debit Abebe's wallet (liability ↓).** The 300 ETB must stop being
  spendable the moment the request is accepted. Otherwise Abebe could request
  the withdrawal and then spend the same 300 on a payment before the payout
  goes out: a double spend. Debiting the wallet drops its balance to 700.
- **Line 2, credit Pending withdrawals (liability ↑).** The platform still owes
  the 300 ETB, just in a different form: it now owes a bank payout instead of
  a spendable wallet balance. So the money moves from one liability to
  another; it does not disappear.
- **The equation stays in balance:** Assets 0 = Liabilities −300 + 300. Total
  liabilities stay at 1,000. Only their breakdown changes: 700 in the wallet,
  300 pending payout.

**Why a separate account and not just a "pending" flag:** the pending 300 is
real money the platform owes, so it must be visible in the trial balance. A
dedicated account means every pending payout can be listed, totalled and
aged, and a payout that stays pending too long shows up as a balance that
should have gone to zero. This matches the plan to model holds as postings,
which step 1.02 records as an ADR.

**What happens next:** the 2020:wd-0001 balance always ends at zero, by one of
three paths:

| Outcome | Entry | Effect |
|---|---|---|
| The payout is sent | E03 | Dr 2020 / Cr 1010: cash leaves the bank |
| The payout fails before sending | E04 | Dr 2020 / Cr 2010: the 300 goes back to the wallet |
| The bank returns it after sending | E05 | Cash comes back into 1010 and the wallet is restored |

**Open questions:**

- Should the money sit in a single shared 2020 account or in one sub-account
  per withdrawal? Per withdrawal makes it easy to see which payouts are stuck
  but creates many short-lived accounts. A hold reference on the entry lines
  could give the same visibility with one account.
- Should a withdrawal fee be charged at request time (in this entry) or only
  when the payout succeeds (E03)? Charging at request time means refunding it
  in E04.
- Is there a limit on how long a withdrawal can stay pending before it must be
  sent or cancelled? That limit becomes an alert on the age of 2020 balances.

## E03. Withdrawal: sent

**Scenario:** The afternoon payout batch runs. The partner bank confirms it
has sent 300 ETB from the platform's customer funds account to Abebe's
personal bank account, completing withdrawal `wd-0001` from E02. There is no
fee.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2020:wd-0001 Pending withdrawals | Liability | 300 | |
| 2 | 1010 Bank: customer funds account | Asset | | 300 |
| | **Total** | | **300** | **300** |

**Why:**

- **Line 1, debit Pending withdrawals (liability ↓).** The platform owed Abebe
  a 300 ETB payout. Now that the payout has gone out, that debt is paid, and
  liabilities decrease on the debit side. `2020:wd-0001` returns to zero, which
  is how the books show that this withdrawal is finished.
- **Line 2, credit the bank account (asset ↓).** 300 ETB of cash has left the
  platform's bank account. Assets decrease on the credit side.
- **The equation stays in balance:** Assets −300 = Liabilities −300. Both
  sides shrink together: the platform has less cash and owes less. Equity does
  not change, because paying back money the platform was holding is neither
  income nor expense.
- **Abebe's wallet (2010) is not touched.** It was already reduced in E02.
  Debiting it again here would take the 300 twice. This is the payoff of E02:
  each step moves the money exactly once.

**Balances after E01–E03:**

| Account | Balance | Matches |
|---|---:|---|
| 1010 Bank: customer funds account | 700 Dr | The bank statement shows 700 ETB |
| 2010:cust-abebe Customer wallet | 700 Cr | Abebe's spendable balance |
| 2020:wd-0001 Pending withdrawals | 0 | The withdrawal is complete |

The cash in the bank (700) equals what the platform owes customers (700). For
a wallet provider that is the most important check of all: every birr owed to
a customer is backed by a birr in the bank.

**Open questions:**

- **When exactly is the payout "sent"?** When the platform submits the batch
  file, when the bank accepts it, or when the bank confirms execution? If
  there is a gap of hours or days, money may need to pass through a "payouts
  in transit" clearing account (9xxx) first, so that 1010 always matches the
  bank statement.
- If the bank charges a fee for each outgoing transfer, who pays it? If the
  platform pays, it is an expense line (Dr 5xxx bank charges / Cr 1010). If the
  customer pays, it is booked in E02 as a withdrawal fee (revenue 4xxx).
- Payouts usually run in batches. Is the batch one journal entry with one
  pair of lines per withdrawal, or one entry per withdrawal? One entry per
  withdrawal keeps each withdrawal's history simple. One batch entry matches
  the single debit on the bank statement.

## E04. Withdrawal: failed

**Scenario:** After E03, Abebe's wallet holds 700 ETB. Abebe requests a second
withdrawal of 200 ETB (`wd-0002`), but types the bank account number wrong.
The request is booked exactly like E02:

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-abebe Customer wallet | Liability | 200 | |
| 2 | 2020:wd-0002 Pending withdrawals | Liability | | 200 |
| | **Total** | | **200** | **200** |

When the payout batch runs, the partner bank rejects the transfer before
sending it because the account number does not exist. No money leaves the
platform's bank account. The platform books E04 to give the 200 ETB back:

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2020:wd-0002 Pending withdrawals | Liability | 200 | |
| 2 | 2010:cust-abebe Customer wallet | Liability | | 200 |
| | **Total** | | **200** | **200** |

**Why:**

- **The bank account (1010) is not touched.** The bank rejected the transfer
  before sending it, so the cash never left. The bank statement still shows
  700 ETB, and the books must too.
- **Line 1, debit Pending withdrawals (liability ↓).** The platform no longer
  owes a 200 ETB bank payout, because that payout cannot happen.
  `2020:wd-0002` returns to zero, which closes the withdrawal.
- **Line 2, credit Abebe's wallet (liability ↑).** The platform still owes
  Abebe the 200 ETB. The debt goes back to being a spendable wallet balance,
  so Abebe can use it or request a new withdrawal with the right account
  number.
- **The equation stays in balance:** Assets 0 = Liabilities −200 + 200. As in
  E02, the money only moves between two liabilities. Total liabilities stay
  at 700.

**A new entry, not an edit of the request.** The E02-style request for
`wd-0002` is not deleted or changed. It was a valid request at the time, so it
stays in history, and E04 is a separate entry that records what happened next.
The ledger can then answer "what happened to withdrawal `wd-0002`?" from its
own records: requested, then failed, with the reason.

**Failed is not the same as reversed.** A reversal (E13) corrects an entry
that was booked by mistake. Nothing was booked by mistake here: the request
was real and the payout genuinely failed. E04 is a normal business event with
its own reason code (e.g. `invalid_account`), even though its lines mirror the
request.

**Balances after E01–E04:**

| Account | Balance | Matches |
|---|---:|---|
| 1010 Bank: customer funds account | 700 Dr | The bank statement shows 700 ETB |
| 2010:cust-abebe Customer wallet | 700 Cr | Abebe's spendable balance |
| 2020:wd-0001 Pending withdrawals | 0 | Sent in E03 |
| 2020:wd-0002 Pending withdrawals | 0 | Failed in E04 |

Cash in the bank still equals what customers are owed.

**Open questions:**

- **Duplicate failure notices.** Banks sometimes report the same failure
  twice. The second notice must not credit the wallet again. If
  `2020:wd-0002` may never go below zero, the second posting is rejected
  automatically; an idempotency key on the bank's failure reference is the
  other safeguard (step 1.11).
- **Fee refund.** If a withdrawal fee was charged at request time, should it
  be refunded when the payout fails? Most providers refund it when the failure
  is not the customer's fault, but an invalid account number arguably is.
- **Retries.** Is a failed withdrawal ever retried automatically (for example,
  after a temporary bank outage)? If so, does the retry keep `wd-0002` pending
  instead of booking E04, and how long before it gives up?

## E05. Withdrawal: returned by bank

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**
