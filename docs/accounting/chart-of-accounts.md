# Chart of Accounts v0

[Accounting model](README.md) · Used by: [worked entries](entries/README.md)

Codes and types only. Balances and flags come in step 1.05.

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
