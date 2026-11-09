# Worked Journal Entries

[Accounting model](../README.md) · [Chart of accounts](../chart-of-accounts.md)

Each entry is booked from the **platform's** point of view. IDs (E01–E18) are
stable and are referenced from code and tests in later steps.

The entries form one continuous story. Read them in order: each one starts
from the balances the previous one left.

## Cast

| Name | Role | Accounts |
|---|---|---|
| Abebe | Customer | `2010:cust-abebe` |
| Hana | Customer | `2010:cust-hana`, offline device `dev-hana-01` |
| Merkato Coffee | Merchant | `2030:merch-coffee` |
| The platform | Runs the wallet and holds the bank accounts | 1010, 1020, revenue and expense accounts |

## Opening position (OB)

Before E01, the platform's owners put 5,000 ETB of their own money into the
platform's operating bank account. This is the platform's capital, which it
uses to pay its own costs (see E14). It is not customer money.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 1020 Bank: operating account | Asset | 5,000 | |
| 2 | 3010 Owner's capital | Equity | | 5,000 |
| | **Total** | | **5,000** | **5,000** |

The platform keeps **two bank accounts**: the customer funds account (1010)
holds only money owed to customers and merchants, plus fees not yet swept;
the operating account (1020) holds the platform's own money. Keeping them
apart means customer money can never pay the platform's bills.

## Index

| ID | Entry |
|---|---|
| E01 | [Deposit via bank](deposits-and-withdrawals.md#e01-deposit-via-bank) |
| E02 | [Withdrawal: requested](deposits-and-withdrawals.md#e02-withdrawal-requested) |
| E03 | [Withdrawal: sent](deposits-and-withdrawals.md#e03-withdrawal-sent) |
| E04 | [Withdrawal: failed](deposits-and-withdrawals.md#e04-withdrawal-failed) |
| E05 | [Withdrawal: returned by bank](deposits-and-withdrawals.md#e05-withdrawal-returned-by-bank) |
| E06 | [P2P transfer](payments.md#e06-p2p-transfer) |
| E07 | [QR merchant payment with fee](payments.md#e07-qr-merchant-payment-with-fee) |
| E08 | [Card tap payment](payments.md#e08-card-tap-payment) |
| E09 | [Fingerprint payment](payments.md#e09-fingerprint-payment) |
| E10 | [Payment request paid](payments.md#e10-payment-request-paid) |
| E11 | [Full refund](refunds-and-corrections.md#e11-full-refund) |
| E12 | [Partial refund](refunds-and-corrections.md#e12-partial-refund) |
| E13 | [Reversal](refunds-and-corrections.md#e13-reversal) |
| E14 | [Admin adjustment](refunds-and-corrections.md#e14-admin-adjustment) |
| E15 | [Merchant settlement payout](settlement.md#e15-merchant-settlement-payout) |
| E16 | [Offline escrow reservation](offline-escrow.md#e16-offline-escrow-reservation) |
| E17 | [Offline receipt settlement](offline-escrow.md#e17-offline-receipt-settlement) |
| E18 | [Escrow return on expiry](offline-escrow.md#e18-escrow-return-on-expiry) |

## Entry template

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
