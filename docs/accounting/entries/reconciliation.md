# Reconciliation (E32–E35)

[All entries](README.md) · [Chart of accounts](../chart-of-accounts.md) · [Clearing and suspense policy](../clearing-and-suspense.md) · [ADR-0020](../../adr/0020-reconciliation.md)

When reconciliation finds that the bank statement and 1010 disagree, the
first entry makes 1010 equal the bank again and parks the difference in a
suspense item. The second entry, once someone has found the cause, closes
the item. These examples stand on their own, outside the running story:
each pair starts from the balances it names.

| The bank shows | First entry | Template | Closed by |
|---|---|---|---|
| **More** than the ledger | Dr 1010 / Cr `9010:sus-…` | `deposit_unmatched.v1` (E19) | `suspense_resolve.v1` (E20) |
| **Less** than the ledger | Dr `9040:sbd-…` / Cr 1010 | `bank_debit_unmatched.v1` (E32, E34) | `bank_debit_resolve.v1` (E33, E35) |

## E32. A bank charge nobody booked

**Scenario:** The partner bank's statement for the month shows a 15 ETB
charge taken from the customer funds account. Nobody booked it, so 1010 in
the ledger is 15 ETB higher than the bank. Two days later the daily
reconciliation reports a `missing_in_ledger` break: the bank shows money
leaving that the ledger does not. Operations parks the difference in
suspense item `sbd-0001`.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 9040:sbd-0001 Suspense: unexplained bank debits | Asset | 15 | |
| 2 | 1010 Bank: customer funds account | Asset | | 15 |
| | **Total** | | **15** | **15** |

**Why:**

- **Line 2, credit the bank account (asset ↓).** The money has left the
  bank. 1010 must equal the statement again, whatever the cause turns out
  to be.
- **Line 1, debit the suspense item (asset ↑).** Until someone knows why it
  left, the ledger records it as a claim to be explained, not as an
  expense. Booking it straight to an expense would decide the cause before
  anyone looked.
- **The equation stays in balance:** one asset down, another up.
- **9040 is not cash.** The safeguarding check counts only 1010, so the
  15 ETB shortfall shows there until the item is closed.

## E33. The suspense item is a bank charge

**Scenario:** After E32, operations checks the bank's fee schedule: the
15 ETB is the monthly account charge. The item is closed to bank charges.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 5020 Bank charges | Expense | 15 | |
| 2 | 9040:sbd-0001 Suspense: unexplained bank debits | Asset | | 15 |
| | **Total** | | **15** | **15** |

**Why:**

- **Line 1, debit bank charges (expense ↑).** The platform pays the bank's
  fees. It is the platform's cost, never a customer's.
- **Line 2, credit the suspense item (asset ↓).** `9040:sbd-0001` returns
  to zero, which closes the item. The template refuses any amount other
  than its balance.
- **The charge came out of the customer funds account.** The platform
  owes that account 15 ETB from its own money: a transfer from the
  operating account (1020) to 1010, booked when it is made.

## E34. A deposit booked twice

**Scenario:** A client booked Abebe's 200 ETB deposit (E01) twice, with two
idempotency keys. The bank received 200 ETB once. Reconciliation matches one
of the two ledger lines to the statement and reports the other as
`missing_in_statement`: the ledger shows 200 ETB in 1010 that the bank does
not. Operations parks it in suspense item `sbd-0002`.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 9040:sbd-0002 Suspense: unexplained bank debits | Asset | 200 | |
| 2 | 1010 Bank: customer funds account | Asset | | 200 |
| | **Total** | | **200** | **200** |

**Why:** the same two lines as E32. Reconciliation does not need to know the
cause to put 1010 right: the bank has 200 ETB less than the ledger.

## E35. The duplicate is taken back from the wallet

**Scenario:** After E34, the client's records show the second deposit was a
replay of the first. Abebe's wallet was credited 200 ETB that never arrived,
and still holds it. The item is closed against the wallet.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-abebe Customer wallet | Liability | 200 | |
| 2 | 9040:sbd-0002 Suspense: unexplained bank debits | Asset | | 200 |
| | **Total** | | **200** | **200** |

**Why:**

- **Line 1, debit the wallet (liability ↓).** The platform does not owe
  Abebe the second 200 ETB: it never arrived.
- **Line 2, credit the suspense item (asset ↓).** The item closes.
- **Taken together, E34 and E35 undo the duplicate** exactly as a reversal
  would, but 1010 agreed with the bank from the moment of E34, before
  anyone knew whose wallet held the money.
- **The wallet may not hold it any more.** The posting rules refuse to take
  the wallet below zero (INV-8); the item then stays open and ages until
  the client recovers the money, and a person decides.

The cause can also be the bank's mistake: when the bank returns money it
took in error, the statement shows it arriving, and the item is closed
against 1010 (Dr 1010 / Cr `9040:sbd-…`).
