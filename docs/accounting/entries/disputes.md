# Disputes (E21–E24)

[All entries](README.md) · [Chart of accounts](../chart-of-accounts.md) · [Refunds and disputes policy](../refunds-and-disputes.md)

A dispute is a payer's claim that a payment should not stand: the goods
never came, or the payer says they never paid. While the platform decides,
the disputed money is taken out of the merchant's reach, but it is not given
to the payer yet. These entries continue from E20: Hana's wallet holds
670 ETB and Merkato Coffee's balance is 58.80 ETB.

## E21. Dispute opened

**Scenario:** Hana disputes her fingerprint payment at Merkato Coffee (E09,
50 ETB, of which the merchant received 49): she says the order never
arrived. The platform opens dispute `dsp-0001` and holds the merchant's
49 ETB.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2030:merch-coffee Merchant balance | Liability | 49 | |
| 2 | 2060:dsp-0001 Disputed merchant funds | Liability | | 49 |
| | **Total** | | **49** | **49** |

**Why:**

- **Line 1, debit the merchant balance (liability ↓).** Merkato Coffee can no
  longer spend or be paid out the 49 ETB while the dispute is open.
- **Line 2, credit the dispute item (liability ↑).** The platform still owes
  the 49 ETB, to whichever side the dispute is decided for. Until then, it is
  owed to neither.
- **Only what the merchant received is held.** The 1 ETB fee stays in fee
  revenue (4010); if the payer wins, the platform keeps it (see the policy).
- **The equation stays in balance:** Assets 0 = Liabilities −49 + 49.

## E22. Dispute resolved for the payer

**Scenario:** Merkato Coffee cannot show that the order was delivered. The
platform decides `dsp-0001` for Hana.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2060:dsp-0001 Disputed merchant funds | Liability | 49 | |
| 2 | 2010:cust-hana Customer wallet | Liability | | 49 |
| | **Total** | | **49** | **49** |

**Why:**

- **Line 1, debit the dispute item (liability ↓).** It returns to zero: the
  dispute is closed.
- **Line 2, credit Hana's wallet (liability ↑).** Hana gets back what the
  merchant received for E09.
- **It is a refund of E09.** The entry names E09, and the ledger counts it
  against E09's refund limit, so a later refund of E09 can give back only
  the 1 ETB that is left, never the 49 again.
- **The equation stays in balance:** Assets 0 = Liabilities −49 + 49.

## E23. Dispute opened

**Scenario:** Abebe disputes the rest of the QR payment at Merkato Coffee
(E07), part of which was already refunded in E12. Merkato Coffee's balance is
down to 9.80 ETB after E21, so dispute `dsp-0002` holds 9.80 ETB.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2030:merch-coffee Merchant balance | Liability | 9.80 | |
| 2 | 2060:dsp-0002 Disputed merchant funds | Liability | | 9.80 |
| | **Total** | | **9.80** | **9.80** |

**Why:** the same as E21, for a different payment. Each dispute has its own
sub-account.

## E24. Dispute resolved for the merchant

**Scenario:** Merkato Coffee shows a delivery receipt signed by Abebe. The
platform decides `dsp-0002` for the merchant.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2060:dsp-0002 Disputed merchant funds | Liability | 9.80 | |
| 2 | 2030:merch-coffee Merchant balance | Liability | | 9.80 |
| | **Total** | | **9.80** | **9.80** |

**Why:**

- **The held money goes back to the merchant** and `2060:dsp-0002` returns to
  zero. E21 and E23 together with their resolutions leave the merchant
  exactly where a decision for each side says it should be.
- **It is not a refund.** Nothing is given back to the payer, so E07's refund
  limit is unchanged.

**Open questions:**

- **Partial decisions.** Can a dispute be decided partly for each side? Not
  in this version: each dispute is decided wholly for one side. A split
  would be two disputes.
- **Time limits.** How long may a dispute stay open? The ageing report lists
  open disputes; the time limit is a business rule (see the policy).
