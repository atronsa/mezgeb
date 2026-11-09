# Mezgeb Accounting Model

> **Status:** Draft (Week 1). Roadmap: [phase-1.md § Week 1](roadmap/phase-1.md#week-1-accounting-foundations-and-worked-examples)
>
> **Reviewer:** _TBD (someone with accounting knowledge)_ · **Reviewed on:** _TBD_

This document books every Atronsa money flow as a double-entry journal entry
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
|  |  |  |  |  |

---

## 3. Worked Journal Entries

### E01. Deposit via bank

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

### E02. Withdrawal: requested

**Scenario:**

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 |  |  |  |  |
| 2 |  |  |  |  |
| | **Total** | | | |

**Why:**

**Open questions:**

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

- [ ] Every flow in the Atronsa product list has a worked entry
- [ ] The trial balance balances and every account sits on its normal side
- [ ] Chart of accounts v0 is filled in
- [ ] An accounting reviewer has signed off (name and date at the top)
- [ ] Exit gate: explain, without notes, why a customer deposit *credits* a
      liability and *debits* an asset (record the evidence in `docs/evidence/week-01.md`)
