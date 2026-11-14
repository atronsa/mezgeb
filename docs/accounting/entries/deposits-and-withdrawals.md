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

**Decided in step 2.02** ([clearing and suspense](../clearing-and-suspense.md)):

- **Only final money is booked.** E01 is booked when the partner bank's
  statement shows the credit in 1010, not on an earlier notification, so a
  confirmed deposit needs no clearing step and E01 stays one entry. The
  deposits-in-transit clearing account (9020) stays reserved until a bank
  integration needs it.
- **A deposit with a missing or wrong reference goes to suspense.** The money
  is real but has no owner yet, so it is credited to its own suspense item in
  9010 until it is matched ([E19, E20](suspense.md)).

**Open questions:**

- If the bank charges a fee for incoming transfers, who pays it, the customer
  or the platform? If the platform pays, it is an expense (5xxx) booked in a
  separate line. **Decided in step 2.03** ([ADR-0014](../../adr/0014-payment-channels-and-fees.md)): no deposit
  fees in this version; 5020 Bank charges stays reserved until a bank charges
  one.

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

**What happens next:** the 2020:wd-0001 balance always ends at zero. The
payout is sent, and the bank then either confirms it (E03) or rejects it
(E04). A payout the bank confirmed can still come back later (E05):

| Outcome | Entries | Effect |
|---|---|---|
| Sent, then confirmed | E03 | Dr 2020 / Cr 9030 when sent, then Dr 9030 / Cr 1010: cash leaves the bank |
| Sent, then rejected | E04 | Dr 2020 / Cr 9030 when sent, then Dr 9030 / Cr 2010: the money goes back to the wallet |
| Confirmed, then returned | E05 | Cash comes back into 1010 and the wallet is restored |

**Decided in step 2.02:** one sub-account per withdrawal (`2020:wd-…`), so a
stuck payout shows up as a balance that should have gone to zero, and the
[ageing report](../clearing-and-suspense.md#ageing) lists every pending
withdrawal with its age. A withdrawal is booked with ordinary entries rather
than a ledger hold, because a hold expires by itself and would give the money
back even after the payout had gone to the bank.

**Open questions:**

- Should a withdrawal fee be charged at request time (in this entry) or only
  when the payout succeeds (E03)? Charging at request time means refunding it
  in E04. **Decided in step 2.03** ([ADR-0014](../../adr/0014-payment-channels-and-fees.md)): no withdrawal fees
  in this version. A fee would be a new version of `withdrawal_request` with
  a fee line to 4020, never a separate entry.

## E03. Withdrawal: sent and confirmed

**Scenario:** The afternoon payout batch runs. The platform submits the
300 ETB payout for withdrawal `wd-0001` (E02) to the partner bank. The next
morning, the bank's statement confirms that 300 ETB left the customer funds
account for Abebe's personal bank account. There is no fee.

When the payout is submitted, it moves from pending to in transit:

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2020:wd-0001 Pending withdrawals | Liability | 300 | |
| 2 | 9030:wdt-0001 Clearing: withdrawals in transit | Liability | | 300 |
| | **Total** | | **300** | **300** |

When the bank confirms it, the cash leaves:

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 9030:wdt-0001 Clearing: withdrawals in transit | Liability | 300 | |
| 2 | 1010 Bank: customer funds account | Asset | | 300 |
| | **Total** | | **300** | **300** |

**Why:**

- **(a) Sent: the money is in flight.** Once the payout is with the bank, the
  platform can no longer cancel it, but the cash has not left 1010 either: the
  bank may still reject it. The debt moves from "pending payout" (2020) to
  "payout in transit" (9030). Both are liabilities, so total liabilities stay
  at 1,000, and `2020:wd-0001` returns to zero.
- **(b) Confirmed: the debt is paid.** The bank's statement shows the 300
  leaving, so the books do too: 1010 decreases (asset ↓, credit), and the
  in-transit debt is paid (liability ↓, debit). `9030:wdt-0001` returns to
  zero, which is how the books show that this withdrawal is finished.
- **1010 always matches the bank statement.** Between (a) and (b), the bank
  still shows 1,000 ETB, and so does 1010. Without the clearing account, the
  books would either show the cash gone a day early, or show nothing in
  flight at all.
- **The equation stays in balance:** (a) Assets 0 = Liabilities −300 + 300;
  (b) Assets −300 = Liabilities −300. Equity does not change, because paying
  back money the platform was holding is neither income nor expense.
- **Abebe's wallet (2010) is not touched.** It was already reduced in E02.
  Debiting it again here would take the 300 twice. Each step moves the money
  exactly once.

**Balances after E01–E03:**

| Account | Balance | Matches |
|---|---:|---|
| 1010 Bank: customer funds account | 700 Dr | The bank statement shows 700 ETB |
| 2010:cust-abebe Customer wallet | 700 Cr | Abebe's spendable balance |
| 2020:wd-0001 Pending withdrawals | 0 | The payout was sent |
| 9030:wdt-0001 Clearing: withdrawals in transit | 0 | The bank confirmed it |

The cash in the bank (700) equals what the platform owes customers (700). For
a wallet provider that is the most important check of all: every birr owed to
a customer is backed by a birr in the bank.

**Open questions:**

- If the bank charges a fee for each outgoing transfer, who pays it? If the
  platform pays, it is an expense line (Dr 5xxx bank charges / Cr 1010). If the
  customer pays, it is booked in E02 as a withdrawal fee (revenue 4xxx).
  **Decided in step 2.03** ([ADR-0014](../../adr/0014-payment-channels-and-fees.md)): none in this version.
- Payouts usually run in batches. Is the batch one journal entry with one
  pair of lines per withdrawal, or one entry per withdrawal? Step 2.02 books
  one entry per withdrawal, which keeps each withdrawal's history simple;
  reconciliation (step 2.15) matches them to the single debit on the bank
  statement.

## E04. Withdrawal: rejected by the bank

**Scenario:** After E03, Abebe's wallet holds 700 ETB. Abebe requests a second
withdrawal of 200 ETB (`wd-0002`), but types the bank account number wrong.
The request is booked exactly like E02:

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-abebe Customer wallet | Liability | 200 | |
| 2 | 2020:wd-0002 Pending withdrawals | Liability | | 200 |
| | **Total** | | **200** | **200** |

The payout batch submits it, exactly like E03(a):

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2020:wd-0002 Pending withdrawals | Liability | 200 | |
| 2 | 9030:wdt-0002 Clearing: withdrawals in transit | Liability | | 200 |
| | **Total** | | **200** | **200** |

The partner bank rejects the transfer because the account number does not
exist. No money leaves the platform's bank account. The platform books E04 to
give the 200 ETB back:

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 9030:wdt-0002 Clearing: withdrawals in transit | Liability | 200 | |
| 2 | 2010:cust-abebe Customer wallet | Liability | | 200 |
| | **Total** | | **200** | **200** |

**Why:**

- **The bank account (1010) is not touched.** The bank rejected the transfer,
  so the cash never left. The bank statement still shows 700 ETB, and the
  books must too.
- **Line 1, debit the clearing account (liability ↓).** The platform no
  longer owes a 200 ETB payout in transit, because that payout will not
  happen. `9030:wdt-0002` returns to zero, which closes the withdrawal.
- **Line 2, credit Abebe's wallet (liability ↑).** The platform still owes
  Abebe the 200 ETB. The debt goes back to being a spendable wallet balance,
  so Abebe can use it or request a new withdrawal with the right account
  number. The wallet gets back exactly the 200 that was requested: the
  template refuses any other amount.
- **The equation stays in balance:** Assets 0 = Liabilities −200 + 200. As in
  E02, the money only moves between two liabilities. Total liabilities stay
  at 700.

**A new entry, not an edit of the request.** The request and the payout for
`wd-0002` are not deleted or changed. They were valid at the time, so they
stay in history, and E04 is a separate entry that records what happened next.
The ledger can then answer "what happened to withdrawal `wd-0002`?" from its
own records: requested, sent, then rejected, with the reason.

**Failed is not the same as reversed.** A reversal (E13) corrects an entry
that was booked by mistake. Nothing was booked by mistake here: the request
was real and the payout genuinely failed. E04 is a normal business event with
its own reason code (e.g. `invalid_account`).

**Balances after E01–E04:**

| Account | Balance | Matches |
|---|---:|---|
| 1010 Bank: customer funds account | 700 Dr | The bank statement shows 700 ETB |
| 2010:cust-abebe Customer wallet | 700 Cr | Abebe's spendable balance |
| 2020:wd-0001, 2020:wd-0002 Pending withdrawals | 0 | Both were sent |
| 9030:wdt-0001 Clearing: withdrawals in transit | 0 | Confirmed in E03 |
| 9030:wdt-0002 Clearing: withdrawals in transit | 0 | Rejected in E04 |

Cash in the bank still equals what customers are owed.

**Duplicate notices are refused by the ledger.** Banks sometimes report the
same outcome twice. Once `9030:wdt-0002` is back at zero, a second rejection
would take it below zero, which the posting rules refuse (INV-8). For the
same reason, a confirmation after a rejection, or a rejection after a
confirmation, is refused: each withdrawal has exactly one outcome.

**Open questions:**

- **Fee refund.** If a withdrawal fee was charged at request time, should it
  be refunded when the payout fails? Most providers refund it when the failure
  is not the customer's fault, but an invalid account number arguably is.
  Open until withdrawal fees exist ([ADR-0014](../../adr/0014-payment-channels-and-fees.md)).
- **Cancelled before sending.** A request the platform cancels before the
  batch runs (for example, after a compliance check) would move the money
  from 2020 straight back to 2010. No template books that yet.
- **Retries.** Is a failed withdrawal ever retried automatically (for example,
  after a temporary bank outage)? If so, the retry is a new withdrawal with
  its own sub-accounts.

## E05. Withdrawal: returned by bank

**Scenario:** The 300 ETB payout from E03 (`wd-0001`) did leave the
platform's bank account. Three days later, Abebe's bank sends it back because
the receiving account has been closed. The partner bank credits the 300 ETB
to the platform's customer funds account, with a return notice that
references the original transfer. There is no return fee.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 1010 Bank: customer funds account | Asset | 300 | |
| 2 | 2010:cust-abebe Customer wallet | Liability | | 300 |
| | **Total** | | **300** | **300** |

**Why:**

- **Line 1, debit the bank account (asset ↑).** Unlike E04, the cash really
  did leave in E03, and now it has really come back. The platform has 300 ETB
  more cash, so the asset increases on the debit side.
- **Line 2, credit Abebe's wallet (liability ↑).** The money still belongs to
  Abebe. Since the payout could not be delivered, the platform owes it to
  Abebe again, as a spendable wallet balance.
- **The equation stays in balance:** Assets +300 = Liabilities +300. Both sides
  grow back by the amount E03 reduced them. The lines look like E01, because
  economically the money is arriving again.
- **Neither `2020:wd-0001` nor `9030:wdt-0001` is reopened.** That
  withdrawal closed at zero in E03, and the payout was correctly booked: it
  did happen. E05 records a new event on a later date and references
  `wd-0001` with a reason code (e.g. `account_closed`), so the withdrawal's
  history reads: requested, sent, confirmed, returned. Because the lines are
  the same as E01's, E05 is booked with the same template,
  `deposit_confirmed.v1`.

**Returned is not the same as failed or reversed:**

| | E04 Failed | E05 Returned | E13 Reversal |
|---|---|---|---|
| Did cash leave the bank? | No | Yes, then came back | Depends on the entry |
| Bank account (1010) | Not touched | Debited on return | Mirror of the original |
| Was the original entry wrong? | No | No | **Yes** |
| Lines | Dr 9030 / Cr 2010 | Dr 1010 / Cr 2010 | Mirror of the original |

E03 is not reversed, because it was correct: the bank statement shows 300 ETB
leaving on day 1 and 300 ETB arriving on day 4. The books must show the same
two movements, or the reconciliation against the bank statement (step 2.15)
would never match.

**Balances after E01–E05:**

| Account | Balance | Matches |
|---|---:|---|
| 1010 Bank: customer funds account | 1,000 Dr | The bank statement: +1,000, −300, +300 |
| 2010:cust-abebe Customer wallet | 1,000 Cr | Abebe's spendable balance |
| 2020:wd-0001, 9030:wdt-0001 | 0 | Sent and confirmed in E03, returned in E05 |
| 2020:wd-0002, 9030:wdt-0002 | 0 | Rejected in E04 |

Cash in the bank still equals what customers are owed.

**Decided in step 2.02:** a return whose notice does not clearly reference
the original transfer goes to suspense, like any unidentified deposit
([E19](suspense.md)), not straight to a wallet.

**Open questions:**

- **Return fees.** If the bank deducts a fee and returns only 290 ETB, who
  absorbs the 10 ETB? If the customer does, the wallet is credited 290. If the
  platform does, the wallet gets 300 and a separate line books 10 ETB as an
  expense (5xxx).
- **Fraud review.** Should returned funds go straight back to the wallet, or
  be held until checked? Repeated returns can be a sign of money laundering
  through closed or mule accounts.
- **Late returns.** Is there a time limit after which a return is no longer
  accepted automatically and needs manual review?
