# Merchant Settlement (E15)

[All entries](README.md) · [Chart of accounts](../chart-of-accounts.md)

## E15. Merchant settlement payout

**Scenario:** At the end of the day, the platform pays Merkato Coffee
everything it is owed. After the payments (E07–E09) and refunds (E11, E12),
the merchant balance is 107.80 ETB. Settlement works like a withdrawal (E02,
E03): first the amount is committed to a payout, then the bank sends it.

**(a) Settlement created** (settlement `stl-0001`):

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2030:merch-coffee Merchant balance | Liability | 107.80 | |
| 2 | 2040:stl-0001 Pending merchant settlements | Liability | | 107.80 |
| | **Total** | | **107.80** | **107.80** |

**(b) Settlement sent:** the partner bank confirms the transfer to Merkato
Coffee's bank account:

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2040:stl-0001 Pending merchant settlements | Liability | 107.80 | |
| 2 | 1010 Bank: customer funds account | Asset | | 107.80 |
| | **Total** | | **107.80** | **107.80** |

**Why:**

- **(a) moves the debt, not the cash.** As in E02, the platform still owes the
  merchant 107.80 ETB, but now as a bank payout instead of a balance. Moving it
  out of 2030 first means that a refund arriving while the payout is in
  progress cannot use money that is already on its way to the merchant's
  bank.
- **(b) pays the debt.** As in E03, cash leaves the bank (asset ↓) and the
  platform no longer owes the payout (liability ↓). `2040:stl-0001` returns to
  zero.
- **Merchant settlements use their own pending account (2040).** They are
  reported, reconciled and approved differently from customer withdrawals:
  in batches, on a schedule, and often against a settlement statement sent
  to the merchant.
- **The amount is the merchant's net balance.** Fees were already deducted at
  payment time (E07), so settlement pays out exactly what the ledger says the
  merchant is owed, with no fee calculation at settlement.

**Failures and returns** follow E04 and E05: a payout rejected before sending
moves the money from 2040 back to 2030, and a payout returned after sending
debits 1010 and credits 2030.

**Open questions:**

- **Settlement delay.** Should recent payments be held back for a few days
  (for example, T+1 or T+2) to leave time for refunds and disputes? Then
  settlement pays only the part of the balance older than the delay.
- **Minimum payout.** Is there a minimum amount below which the balance rolls
  over to the next settlement?
- **Negative balances.** If refunds exceed payments, the merchant owes the
  platform. Is that carried forward, deducted from the next settlement, or
  collected separately?
- **Settlement fees.** Are any fees charged at settlement (for example, for
  instant payouts)? Those would be a third line to 4xxx revenue in (a).
