# Chart of Accounts v0

[Accounting model](README.md) · Used by: [worked entries](entries/README.md)

Every account a worked entry posts to is listed here, and every active account
here is used by at least one worked entry. The same chart is a data file the
engine loads, [`charts/wallet-provider.toml`](../../charts/wallet-provider.toml).
A test (`crates/core/tests/wallet_provider_chart.rs`) fails if this page, the
data file and the worked entries ever disagree, so change all three together.

## Code scheme

Codes have four digits. The first digit gives the type, and the second groups
related accounts.

| Range | Type | Groups |
|---|---|---|
| 1000–1999 | Assets | 10xx bank accounts |
| 2000–2999 | Liabilities | 20xx money owed to customers and merchants |
| 3000–3999 | Equity | 30xx capital |
| 4000–4999 | Revenue | 40xx fees |
| 5000–5999 | Expenses | 50xx operating costs and losses |
| 9000–9999 | Suspense and clearing | 90xx money waiting to be matched or settled. Each account still has its own type (asset or liability). |

**Rules:**

- New accounts take the next free code in their group, in steps of 10, which
  leaves room to insert related accounts later.
- A code is never reused, even after its account is closed. Old entries keep
  pointing to the account they were posted to.
- The normal side follows from the type: assets and expenses are debit,
  liabilities, equity and revenue are credit. Suspense and clearing accounts
  follow their own type, not their range.

## Control and system accounts

| Kind | Meaning | Example |
|---|---|---|
| **Control** | A family of sub-accounts, one per customer, merchant, withdrawal, settlement, voucher or suspense item. Entries post to a sub-account, never to the control account itself. The control account's balance is the sum of its sub-accounts. | `2010:cust-abebe` |
| **System** | A single account. Entries post to it directly. | `4010` |

Sub-accounts are written `<code>:<prefix>-<id>`. The prefix says what the
sub-account belongs to:

| Prefix | Belongs to | Used with |
|---|---|---|
| `cust` | A customer | 2010 |
| `wd` | A withdrawal | 2020 |
| `merch` | A merchant | 2030 |
| `stl` | A merchant settlement | 2040 |
| `vch` | An offline voucher | 2050 |
| `dsp` | A dispute | 2060 |
| `sus` | An unidentified payment (suspense item) | 9010 |
| `wdt` | A withdrawal payout in transit | 9030 |

## Accounts

**Status:** *Active* accounts are used by at least one worked entry. *Reserved*
accounts are needed by an open question in the entries; their code and type
are fixed now, and the step named decides how they are posted.

### Assets

| Code | Name | Kind | Normal side | Status | Used in | Purpose |
|---|---|---|---|---|---|---|
| 1010 | Bank: customer funds account | System | Dr | Active | E01, E03, E05, E13, E14, E15, E19 | The account at the partner bank that holds customer and merchant money, plus fees not yet swept. |
| 1020 | Bank: operating account | System | Dr | Active | OB, E14 | The platform's own money. Never mixed with customer funds. |

### Liabilities

| Code | Name | Kind | Normal side | Status | Used in | Purpose |
|---|---|---|---|---|---|---|
| 2010 | Customer wallets | Control | Cr | Active | E01, E02, E04–E14, E16, E18, E20, E22 | Money the platform owes each customer, spendable online. |
| 2020 | Pending withdrawals | Control | Cr | Active | E02, E03, E04 | Customer money committed to a bank payout that has not been sent yet. Each sub-account returns to zero. |
| 2030 | Merchant balances | Control | Cr | Active | E07–E09, E11, E12, E15, E17, E21, E23, E24 | Money owed to each merchant for payments received, net of fees. |
| 2040 | Pending merchant settlements | Control | Cr | Active | E15 | Merchant money committed to a settlement payout that has not been sent yet. Each sub-account returns to zero. |
| 2050 | Offline escrow | Control | Cr | Active | E16, E17, E18 | Customer money reserved for offline spending on one device. Each sub-account returns to zero when its voucher expires. |
| 2060 | Disputed merchant funds | Control | Cr | Active | E21–E24 | Merchant money held while a payer's dispute is open, one sub-account per dispute. Each sub-account returns to zero ([refunds and disputes](refunds-and-disputes.md)). |

### Equity

| Code | Name | Kind | Normal side | Status | Used in | Purpose |
|---|---|---|---|---|---|---|
| 3010 | Owner's capital | System | Cr | Active | OB | Money the platform's owners put into the business. |

### Revenue

| Code | Name | Kind | Normal side | Status | Used in | Purpose |
|---|---|---|---|---|---|---|
| 4010 | Payment fee revenue | System | Cr | Active | E07–E09, E11, E12, E17 | Fees earned on merchant payments, online and offline. Reduced by fees given back on refunds. |
| 4020 | Service fee revenue | System | Cr | Reserved (no service fees yet, ADR-0014) | E02, E15 open questions | Fees charged to customers or merchants for services such as withdrawals or instant settlement. |

### Expenses

| Code | Name | Kind | Normal side | Status | Used in | Purpose |
|---|---|---|---|---|---|---|
| 5010 | Customer compensation expense | System | Dr | Active | E14 | Goodwill credits and compensation paid from the platform's own money. |
| 5020 | Bank charges | System | Dr | Reserved (no bank charges yet, ADR-0014) | E01, E03, E05 open questions | Fees the partner bank charges the platform for transfers and returns. |
| 5030 | Offline settlement losses | System | Dr | Reserved (step 2.13) | E18 open questions | Receipts the platform pays out after the escrow behind them was already returned, under the written loss policy. |

### Suspense and clearing

| Code | Name | Type | Kind | Normal side | Status | Used in | Purpose |
|---|---|---|---|---|---|---|---|
| 9010 | Suspense: unidentified funds | Liability | Control | Cr | Active | E19, E20 | Money that has arrived in the bank but cannot yet be matched to a customer or merchant, one sub-account per item. Every item must be cleared to its owner or returned. |
| 9020 | Clearing: deposits in transit | Asset | System | Dr | Reserved (unused, step 2.02) | E01 open questions | Deposits the bank has confirmed but not yet settled into the customer funds account (1010). Unused while the bank notices booked are final. |
| 9030 | Clearing: withdrawals in transit | Liability | Control | Cr | Active | E03, E04 | Customer payouts submitted to the bank and not yet confirmed or rejected, one sub-account per withdrawal. Each sub-account returns to zero. |

How these accounts are used: [clearing and suspense](clearing-and-suspense.md).

## Account groups and the safeguarding check

The customer funds account (1010) backs these liabilities:

| Group | Accounts |
|---|---|
| Customer money | 2010, 2020, 2050, 9030 |
| Merchant money | 2030, 2040, 2060 |
| Unidentified money | 9010 |

At any time, **1010 must be at least the sum of these accounts.** The
difference is fee revenue that has not yet been swept to the operating
account (1020). After E18 the check is 1,402.20 ≥ 1,398.80, a difference of
3.40, which equals the fee revenue earned. This becomes an invariant in step
1.03 and an automated check in step 1.13.

## Open questions

- **Merchant debt.** If refunds exceed a merchant's balance (E11, E15), does
  the merchant sub-account go negative, or does the debt move to a separate
  asset account such as "receivables from merchants"?
- **Fee refunds.** Should refunded fees reduce 4010 directly (as in E11 and
  E12), or go to a contra-revenue account so gross fee income stays visible?
- **Multiple currencies.** All accounts are in ETB today. If another currency
  is added, does each account get one balance per currency, or does each
  currency get its own set of accounts (step 3.08)?
