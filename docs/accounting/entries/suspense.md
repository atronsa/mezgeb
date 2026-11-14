# Suspense (E19–E20)

[All entries](README.md) · [Chart of accounts](../chart-of-accounts.md) · [Clearing and suspense policy](../clearing-and-suspense.md)

Money sometimes arrives in the bank before anyone knows whose it is. These
entries continue from E18.

## E19. Deposit with an unknown reference

**Scenario:** The partner bank's statement shows 50 ETB arriving in the
customer funds account. The payment reference is `HAN4`, which matches no
customer. The money is real, but the platform does not know whose wallet to
credit. It opens suspense item `sus-0001` for it.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 1010 Bank: customer funds account | Asset | 50 | |
| 2 | 9010:sus-0001 Suspense: unidentified funds | Liability | | 50 |
| | **Total** | | **50** | **50** |

**Why:**

- **Line 1, debit the bank account (asset ↑).** The cash is in the bank, so
  it is in the books. Leaving it out until it is matched would make 1010
  disagree with the bank statement.
- **Line 2, credit suspense (liability ↑).** The money belongs to someone
  else: a customer, a merchant, or the sender if it was a mistake. The
  platform owes it, but it does not yet know to whom. It is never revenue.
- **The equation stays in balance:** Assets +50 = Liabilities +50.
- **One sub-account per item.** Each unidentified payment gets its own
  sub-account, so it can be matched, aged and closed on its own, and a
  suspense item can never be resolved for more than it holds.

## E20. Suspense resolved

**Scenario:** The next day, Hana contacts support: she sent 50 ETB and typed
her reference as `HAN4` instead of `HANA`. Support checks the bank details
and resolves `sus-0001` to Hana's wallet.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 9010:sus-0001 Suspense: unidentified funds | Liability | 50 | |
| 2 | 2010:cust-hana Customer wallet | Liability | | 50 |
| | **Total** | | **50** | **50** |

**Why:**

- **Line 1, debit suspense (liability ↓).** The platform no longer owes an
  unknown party. `9010:sus-0001` returns to zero, which closes the item.
- **Line 2, credit Hana's wallet (liability ↑).** It now owes Hana instead.
- **The equation stays in balance:** Assets 0 = Liabilities −50 + 50. Taken
  together, E19 and E20 book exactly what E01 would have, a day later.
- **The whole item is resolved at once.** The template refuses any amount
  other than the item's balance. A payment that really belongs to two people
  is a reason to look again, not to split it.

**Open questions:**

- **Never matched.** If no owner is found, the money goes back to the
  sender: Dr 9010 / Cr 1010. After how long, and who approves it? No template
  books that yet.
- **Resolving to a merchant.** A merchant's payment with a wrong reference
  is resolved the same way, to `2030:merch-…`.
