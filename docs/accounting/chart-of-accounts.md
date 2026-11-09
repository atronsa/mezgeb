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
| 1020 | Bank: operating account | Asset | Dr | The platform's own money, kept separate from customer funds. Introduced in the opening position (OB). |
| 2010 | Customer wallets | Liability | Cr | A control account. Each customer gets their own sub-account (e.g. `2010:cust-abebe`), and the sum of the sub-accounts is the 2010 balance. Introduced in E01. |
| 2020 | Pending withdrawals | Liability | Cr | Money still owed to customers but committed to a payout that has not left the bank yet. One sub-account per withdrawal (e.g. `2020:wd-0001`). Introduced in E02. |
| 2030 | Merchant balances | Liability | Cr | A control account with one sub-account per merchant (e.g. `2030:merch-coffee`): money owed to merchants for payments received, net of fees. Introduced in E07. |
| 2040 | Pending merchant settlements | Liability | Cr | Merchant balances committed to a settlement payout that has not left the bank yet. One sub-account per settlement (e.g. `2040:stl-0001`). Introduced in E15. |
| 2050 | Offline escrow | Liability | Cr | Customer money reserved for offline spending on one device. One sub-account per voucher (e.g. `2050:vch-0001`). Introduced in E16. |
| 3010 | Owner's capital | Equity | Cr | Money the platform's owners put into the business. Introduced in the opening position (OB). |
| 4010 | Payment fee revenue | Revenue | Cr | Fees the platform earns on merchant payments. Introduced in E07. |
| 5010 | Customer compensation expense | Expense | Dr | Goodwill credits and compensation paid to customers from the platform's own money. Introduced in E14. |
|  |  |  |  |  |
