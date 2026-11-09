# Mezgeb Accounting Model

> **Status:** Draft (Week 1). Roadmap: [phase-1.md § Week 1](roadmap/phase-1.md#week-1-accounting-foundations-and-worked-examples)
>
> **Reviewer:** _TBD (someone with accounting knowledge)_ · **Reviewed on:** _TBD_

This document books every common payment flow as a double-entry journal entry
before any code exists. If an entry here is wrong, the code built on it will
be wrong too.

---

## 1. Conventions

- **Currency:** ETB. Amounts are in whole birr in this document for
  readability. The engine stores them as `i64` santim (minor units).
- **Accounting equation:** Assets = Liabilities + Equity + (Revenue − Expenses)
- **Normal balances:**

  | Type | Normal side | Increases with | Decreases with |
  |---|---|---|---|
  | Asset | Debit | Dr | Cr |
  | Liability | Credit | Cr | Dr |
  | Equity | Credit | Cr | Dr |
  | Revenue | Credit | Cr | Dr |
  | Expense | Debit | Dr | Cr |

- **Every entry balances:** the debits equal the credits, per currency.
- **History is immutable:** a correction is a new entry and never an edit.
- **Entry format:** each worked example uses the template below.

<details>
<summary>Entry template</summary>

```markdown
### E00. <Name>

**Scenario:** <who does what, with concrete amounts>

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | **0** | **0** |

**Why:** <why each account moves on this side>

**Open questions:** <anything unsure, to raise with the reviewer>
```

</details>

---

## 2. Chart of Accounts v0

Codes and types only. Balances and flags come in Week 5.

| Range | Type |
|---|---|
| 1000–1999 | Assets |
| 2000–2999 | Liabilities |
| 3000–3999 | Equity |
| 4000–4999 | Revenue |
| 5000–5999 | Expenses |
| 9000–9999 | Suspense / clearing |

| Code | Name | Type | Normal side | Notes |
|---|---|---|---|---|
| 1010 | Bank: customer funds account | Asset | Dr | The platform's account at a partner bank that holds customer money (the "float"). Introduced in E01. |
| 2010 | Customer wallets | Liability | Cr | A control account. Each customer gets their own sub-account (e.g. `2010:cust-abebe`), and the sum of the sub-accounts is the 2010 balance. Introduced in E01. |
| 2020 | Pending withdrawals | Liability | Cr | Money still owed to customers but committed to a payout that has not left the bank yet. One sub-account per withdrawal (e.g. `2020:wd-0001`). Introduced in E02. |
|  |  |  |  |  |

---

## 3. Worked Journal Entries

### E01. Deposit via bank

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

### E02. Withdrawal: requested

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
which Week 2 records as an ADR.

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

### E03. Withdrawal: sent

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E04. Withdrawal: failed

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E05. Withdrawal: returned by bank

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E06. P2P transfer

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E07. QR merchant payment with fee

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| 3 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E08. Card tap payment

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E09. Fingerprint payment

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E10. Payment request paid

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E11. Full refund

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E12. Partial refund

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E13. Reversal

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E14. Admin adjustment

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E15. Merchant settlement payout

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E16. Offline escrow reservation

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E17. Offline receipt settlement

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E18. Escrow return on expiry

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

---

## 4. Trial Balance Worksheet

The balances after applying E01–E18 in order, starting from zero.

| Code | Account | Type | Dr balance | Cr balance | Matches normal side? |
|---|---|---|---:|---:|---|
|  |  |  |  |  |  |
| | **Total** | | | | |

**Check:** total Dr = total Cr → ☐

---

## 5. Week 1 Exit Checklist

- [ ] Every supported money flow has a worked entry
- [ ] The trial balance balances and every account sits on its normal side
- [ ] Chart of accounts v0 is filled in
- [ ] An accounting reviewer has signed off (name and date at the top)
- [ ] Exit gate: explain, without notes, why a customer deposit *credits* a
      liability and *debits* an asset (record the evidence in `docs/evidence/week-01.md`)
