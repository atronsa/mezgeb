# Trial Balance

[Accounting model](README.md) · Inputs: [worked entries](entries/README.md), [chart of accounts](chart-of-accounts.md)

A trial balance lists every account with its closing balance. If every entry
balanced and was posted correctly, the debit balances and credit balances add
up to the same total. This one starts from zero and applies the opening
position (OB) and the worked entries E01–E20 in order: 28 entry tables in all,
because E03, E04, E13, E14 and E15 are booked as more than one entry.

All amounts are in ETB.

## Trial balance after E20

Accounts are listed at sub-account level. A control account's balance is the
sum of its sub-accounts.

| Code | Account | Type | Normal side | Dr balance | Cr balance | On normal side? |
|---|---|---|---|---:|---:|---|
| 1010 | Bank: customer funds account | Asset | Dr | 1,452.20 | | Yes |
| 1020 | Bank: operating account | Asset | Dr | 4,990.00 | | Yes |
| 2010:cust-abebe | Customer wallet: Abebe | Liability | Cr | | 720.00 | Yes |
| 2010:cust-hana | Customer wallet: Hana | Liability | Cr | | 670.00 | Yes |
| 2020:wd-0001 | Pending withdrawal (sent, confirmed, then returned) | Liability | Cr | 0.00 | 0.00 | Zero, as expected |
| 2020:wd-0002 | Pending withdrawal (sent, then rejected) | Liability | Cr | 0.00 | 0.00 | Zero, as expected |
| 2030:merch-coffee | Merchant balance: Merkato Coffee | Liability | Cr | | 58.80 | Yes |
| 2040:stl-0001 | Pending merchant settlement | Liability | Cr | 0.00 | 0.00 | Zero, as expected |
| 2050:vch-0001 | Offline escrow: Hana's voucher | Liability | Cr | 0.00 | 0.00 | Zero, as expected |
| 3010 | Owner's capital | Equity | Cr | | 5,000.00 | Yes |
| 4010 | Payment fee revenue | Revenue | Cr | | 3.40 | Yes |
| 5010 | Customer compensation expense | Expense | Dr | 10.00 | | Yes |
| 9010:sus-0001 | Suspense item (resolved to Hana) | Liability | Cr | 0.00 | 0.00 | Zero, as expected |
| 9030:wdt-0001 | Withdrawal in transit (confirmed) | Liability | Cr | 0.00 | 0.00 | Zero, as expected |
| 9030:wdt-0002 | Withdrawal in transit (rejected) | Liability | Cr | 0.00 | 0.00 | Zero, as expected |
| | **Total** | | | **6,452.20** | **6,452.20** | |

**Result:** total debits equal total credits (6,452.20), and every account
with a balance sits on its normal side. The four reserved accounts in the
chart (4020, 5020, 5030, 9020) were never posted to and are not listed.

## Checks

| # | Check | Calculation | Result |
|---|---|---|---|
| 1 | Every entry balances | 28 of 28 entry tables have equal debits and credits | Pass |
| 2 | Total debits posted = total credits posted | 10,655.60 = 10,655.60 (see [activity by account](#activity-by-account)) | Pass |
| 3 | Debit balances = credit balances | 6,452.20 = 6,452.20 | Pass |
| 4 | Every balance is on its normal side | No asset or expense in credit; no liability, equity or revenue in debit | Pass |
| 5 | Short-lived accounts return to zero | `2020:wd-0001`, `2020:wd-0002`, `2040:stl-0001`, `2050:vch-0001`, `9010:sus-0001`, `9030:wdt-0001` and `9030:wdt-0002` all end at 0 | Pass |
| 6 | Accounting equation | Assets 6,442.20 = Liabilities 1,448.80 + Equity 5,000.00 + (Revenue 3.40 − Expenses 10.00) | Pass |
| 7 | Safeguarding | 1010 (1,452.20) ≥ customer money 1,390.00 + merchant money 58.80 + unidentified money 0 = 1,448.80. The difference, 3.40, equals unswept fee revenue | Pass |
| 8 | Bank accounts are kept apart | 1020 changes only in OB and E14(b), and 1010 never pays the platform's own costs | Pass |

Check 6 in detail:

| Side | Accounts | Amount |
|---|---|---:|
| Assets | 1010 + 1020 | 6,442.20 |
| Liabilities | 2010 (1,390.00) + 2030 (58.80) | 1,448.80 |
| Equity | 3010 | 5,000.00 |
| Revenue − Expenses | 4010 (3.40) − 5010 (10.00) | −6.60 |
| **Right-hand side** | 1,448.80 + 5,000.00 − 6.60 | **6,442.20** |

The platform has a small loss so far (−6.60): it earned 3.40 in fees and
paid 10.00 in compensation. The loss came out of its own money (1020 went
from 5,000 to 4,990), not out of customer funds.

## Activity by account

Total debits and credits posted to each account across all 28 entry tables.
The closing balance is the difference.

| Account | Total Dr | Total Cr | Closing balance | Posted in |
|---|---:|---:|---:|---|
| 1010 | 2,360.00 | 907.80 | 1,452.20 Dr | E01, E03(b), E05, E13(a–c), E14(b), E15(b), E19 |
| 1020 | 5,000.00 | 10.00 | 4,990.00 Dr | OB, E14(b) |
| 2010:cust-abebe | 1,530.00 | 2,250.00 | 720.00 Cr | E01, E02, E04(a, c), E05, E06, E07, E08, E10, E11, E12, E13(a, b), E14(a) |
| 2010:cust-hana | 150.00 | 820.00 | 670.00 Cr | E06, E09, E10, E13(c), E16, E18, E20 |
| 2020:wd-0001 | 300.00 | 300.00 | 0.00 | E02, E03(a) |
| 2020:wd-0002 | 200.00 | 200.00 | 0.00 | E04(a, b) |
| 2030:merch-coffee | 344.00 | 402.80 | 58.80 Cr | E07, E08, E09, E11, E12, E15(a), E17 |
| 2040:stl-0001 | 107.80 | 107.80 | 0.00 | E15(a, b) |
| 2050:vch-0001 | 100.00 | 100.00 | 0.00 | E16, E17, E18 |
| 3010 | 0.00 | 5,000.00 | 5,000.00 Cr | OB |
| 4010 | 3.80 | 7.20 | 3.40 Cr | E07, E08, E09, E11, E12, E17 |
| 5010 | 10.00 | 0.00 | 10.00 Dr | E14(a) |
| 9010:sus-0001 | 50.00 | 50.00 | 0.00 | E19, E20 |
| 9030:wdt-0001 | 300.00 | 300.00 | 0.00 | E03(a, b) |
| 9030:wdt-0002 | 200.00 | 200.00 | 0.00 | E04(b, c) |
| **Total** | **10,655.60** | **10,655.60** | | |

4010 shows debits as well as credits: the fees given back in the refunds
(E11, E12) reduce revenue on the debit side.

## Balance after each entry

Control-account balances after each entry table, to trace any balance back to
the entry that changed it. Debit balances are positive and credit balances
are shown in brackets. Every row sums to zero.

| After | 1010 | 1020 | 2010 | 2020 | 2030 | 2040 | 2050 | 3010 | 4010 | 5010 | 9010 | 9030 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| OB | 0 | 5,000 | 0 | 0 | 0 | 0 | 0 | (5,000) | 0 | 0 | 0 | 0 |
| E01 | 1,000 | 5,000 | (1,000) | 0 | 0 | 0 | 0 | (5,000) | 0 | 0 | 0 | 0 |
| E02 | 1,000 | 5,000 | (700) | (300) | 0 | 0 | 0 | (5,000) | 0 | 0 | 0 | 0 |
| E03(a) sent | 1,000 | 5,000 | (700) | 0 | 0 | 0 | 0 | (5,000) | 0 | 0 | 0 | (300) |
| E03(b) confirmed | 700 | 5,000 | (700) | 0 | 0 | 0 | 0 | (5,000) | 0 | 0 | 0 | 0 |
| E04(a) request | 700 | 5,000 | (500) | (200) | 0 | 0 | 0 | (5,000) | 0 | 0 | 0 | 0 |
| E04(b) sent | 700 | 5,000 | (500) | 0 | 0 | 0 | 0 | (5,000) | 0 | 0 | 0 | (200) |
| E04(c) rejected | 700 | 5,000 | (700) | 0 | 0 | 0 | 0 | (5,000) | 0 | 0 | 0 | 0 |
| E05 | 1,000 | 5,000 | (1,000) | 0 | 0 | 0 | 0 | (5,000) | 0 | 0 | 0 | 0 |
| E06 | 1,000 | 5,000 | (1,000) | 0 | 0 | 0 | 0 | (5,000) | 0 | 0 | 0 | 0 |
| E07 | 1,000 | 5,000 | (900) | 0 | (98) | 0 | 0 | (5,000) | (2) | 0 | 0 | 0 |
| E08 | 1,000 | 5,000 | (700) | 0 | (295) | 0 | 0 | (5,000) | (5) | 0 | 0 | 0 |
| E09 | 1,000 | 5,000 | (650) | 0 | (344) | 0 | 0 | (5,000) | (6) | 0 | 0 | 0 |
| E10 | 1,000 | 5,000 | (650) | 0 | (344) | 0 | 0 | (5,000) | (6) | 0 | 0 | 0 |
| E11 | 1,000 | 5,000 | (850) | 0 | (147) | 0 | 0 | (5,000) | (3) | 0 | 0 | 0 |
| E12 | 1,000 | 5,000 | (890) | 0 | (107.80) | 0 | 0 | (5,000) | (2.20) | 0 | 0 | 0 |
| E13(a) | 1,500 | 5,000 | (1,390) | 0 | (107.80) | 0 | 0 | (5,000) | (2.20) | 0 | 0 | 0 |
| E13(b) | 1,000 | 5,000 | (890) | 0 | (107.80) | 0 | 0 | (5,000) | (2.20) | 0 | 0 | 0 |
| E13(c) | 1,500 | 5,000 | (1,390) | 0 | (107.80) | 0 | 0 | (5,000) | (2.20) | 0 | 0 | 0 |
| E14(a) | 1,500 | 5,000 | (1,400) | 0 | (107.80) | 0 | 0 | (5,000) | (2.20) | 10 | 0 | 0 |
| E14(b) | 1,510 | 4,990 | (1,400) | 0 | (107.80) | 0 | 0 | (5,000) | (2.20) | 10 | 0 | 0 |
| E15(a) | 1,510 | 4,990 | (1,400) | 0 | 0 | (107.80) | 0 | (5,000) | (2.20) | 10 | 0 | 0 |
| E15(b) | 1,402.20 | 4,990 | (1,400) | 0 | 0 | 0 | 0 | (5,000) | (2.20) | 10 | 0 | 0 |
| E16 | 1,402.20 | 4,990 | (1,300) | 0 | 0 | 0 | (100) | (5,000) | (2.20) | 10 | 0 | 0 |
| E17 | 1,402.20 | 4,990 | (1,300) | 0 | (58.80) | 0 | (40) | (5,000) | (3.40) | 10 | 0 | 0 |
| E18 | 1,402.20 | 4,990 | (1,340) | 0 | (58.80) | 0 | 0 | (5,000) | (3.40) | 10 | 0 | 0 |
| E19 | 1,452.20 | 4,990 | (1,340) | 0 | (58.80) | 0 | 0 | (5,000) | (3.40) | 10 | (50) | 0 |
| E20 | 1,452.20 | 4,990 | (1,390) | 0 | (58.80) | 0 | 0 | (5,000) | (3.40) | 10 | 0 | 0 |

Things the table makes visible:

- **Payments never touch the bank.** 1010 stays at 1,000 from E05 to E12,
  while 2010, 2030 and 4010 move between themselves.
- **1010 matches the bank statement while a payout is in flight.** After
  E03(a) and E04(b), 1010 still shows the money, and 9030 shows it as owed in
  transit. 1010 drops only when the bank confirms, in E03(b).
- **Customer funds are backed after every entry but one.** In every row,
  1010 is at least 2010 + 2020 + 2030 + 2040 + 2050 + 9010 + 9030, except
  right after E14(a): the 10.00 goodwill credit leaves 1010 (1,500.00) short
  of the liabilities (1,507.80) by 7.80, until E14(b) moves 10.00 in from
  1020. This is the gap the E14 open question asks about: if (a) and (b)
  post as one entry, the gap never exists.
- **Pending, escrow, clearing and suspense accounts close.** 2020, 2040,
  2050, 9010 and 9030 each go up and back to zero.

## When to update

This worksheet is derived from the entries. If an entry or the opening
position changes, recompute every table here before merging. From step 1.13,
the engine's verification job produces the same trial balance from the posted
ledger, and this worksheet becomes its first test case.
