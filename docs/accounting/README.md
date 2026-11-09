# Accounting Model

> **Status:** Draft. Roadmap step: [1.01 Accounting Foundations](../roadmap/phase-1-core-engine/01-accounting-foundations.md)
>
> **Reviewer:** _TBD (someone with accounting knowledge)_ · **Reviewed on:** _TBD_

This folder books every common payment flow as a double-entry journal entry
before any code exists. If an entry here is wrong, the code built on it will
be wrong too.

| File | Contents |
|---|---|
| [chart-of-accounts.md](chart-of-accounts.md) | Account codes, types and normal sides |
| [entries/](entries/README.md) | Worked journal entries E01–E18, grouped by flow |
| [trial-balance.md](trial-balance.md) | Trial balance after all worked entries |

## Conventions

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
- **Entry format:** every worked entry follows the [entry template](entries/README.md#entry-template).

## Exit checklist (step 1.01)

- [ ] Every supported money flow has a worked entry
- [ ] The trial balance balances and every account sits on its normal side
- [ ] Chart of accounts v0 is filled in
- [ ] An accounting reviewer has signed off (name and date above)
- [ ] Exit gate: explain, without notes, why a customer deposit *credits* a
      liability and *debits* an asset (record the evidence in `docs/evidence/1.01-accounting-foundations.md`)
