# Offline Escrow (E16–E18)

[All entries](README.md) · [Chart of accounts](../chart-of-accounts.md)

Offline payments happen when the payer's device has no connection, so the
ledger cannot check the balance at the moment of payment. Mezgeb handles this
with **escrow**: while the device is online, part of the wallet is moved into
an escrow account bound to that device. Offline, the device can spend only
what is in escrow, so every offline payment is backed by money the ledger
already set aside. The full protocol is in the
[specification §5](../../spec.md#5-offline-sync) and
[architecture §5.8](../../architecture.md).

The examples continue from E15: Hana's wallet holds 680 ETB (including the
500 ETB deposit corrected in E13).

## E16. Offline escrow reservation

**Scenario:** Hana is travelling to an area with poor coverage. While still
online, Hana's phone (`dev-hana-01`) requests an offline allowance of 100 ETB.
The ledger issues a signed voucher (`vch-0001`) valid for 7 days, and moves
100 ETB from Hana's wallet into escrow for that voucher.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-hana Customer wallet | Liability | 100 | |
| 2 | 2050:vch-0001 Offline escrow | Liability | | 100 |
| | **Total** | | **100** | **100** |

**Why:**

- **The money still belongs to Hana**, so escrow is a liability, just like the
  wallet. The platform owes Hana the same 680 ETB in total: 580 in the wallet
  and 100 in escrow.
- **Line 1, debit the wallet (liability ↓).** The 100 ETB must stop being
  spendable online. Otherwise Hana could spend it online *and* offline, and the
  ledger would only find out when the offline receipts sync: a double spend
  that has already happened.
- **Line 2, credit escrow (liability ↑).** The money is now reserved for one
  device and one voucher. Each voucher has its own sub-account, so the ledger
  knows exactly how much each device can still spend.
- **No cash moves.** The bank account is not touched, as in E02.

**Open questions:**

- What is the maximum offline allowance per device trust tier, and per
  customer across all devices?
- Can a voucher be topped up while online, or does each top-up issue a new
  voucher?
- Can Hana cancel the voucher early while online, to get the money back
  before expiry?

## E17. Offline receipt settlement

**Scenario:** At a village market with no signal, Hana pays Merkato Coffee's
offline stall 60 ETB. Hana's phone signs a receipt (counter 1, chained to the
voucher) and the merchant's terminal stores it. The next morning, the
merchant's terminal reconnects and uploads the receipt. The ledger verifies
the signature, voucher, counter and chain, then settles it. The 2% merchant
fee applies, as for E07.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2050:vch-0001 Offline escrow | Liability | 60.00 | |
| 2 | 2030:merch-coffee Merchant balance | Liability | | 58.80 |
| 3 | 4010 Payment fee revenue | Revenue | | 1.20 |
| | **Total** | | **60.00** | **60.00** |

**Why:**

- **The payment comes out of escrow, not the wallet.** The money was set aside
  in E16. Settling from escrow means an offline payment can never make the
  wallet negative, however late the receipt arrives.
- **The rest is the same as E07:** the merchant is owed the price minus the
  fee, and the platform earns the fee. Offline is a different way to
  authorise a payment, not a different kind of payment.
- **Settled exactly once.** Hana's phone may also upload the same receipt
  later. The ledger recognises the same `(device, counter)` and returns
  "duplicate" without posting again. The receipt's own ID acts as the
  idempotency key (step 1.11).
- **Booked when synced, dated when paid.** The entry is posted on the day the
  receipt arrives, but it records the payment's own time (`event_time`), so
  reports can show when the sale actually happened.

**Open questions:**

- If a receipt arrives with counter 3 but counter 2 is missing, does the ledger
  settle 3 and wait for 2, or wait before settling either? (Spec §5.5,
  `WAITING_FOR_GAP`.)
- Should the merchant fee be the same for offline payments, which carry more
  risk for the platform?

## E18. Escrow return on expiry

**Scenario:** Hana's voucher `vch-0001` expires after 7 days. Only 60 ETB of
the 100 ETB allowance was spent (E17). The ledger returns the unspent 40 ETB
to Hana's wallet.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2050:vch-0001 Offline escrow | Liability | 40 | |
| 2 | 2010:cust-hana Customer wallet | Liability | | 40 |
| | **Total** | | **40** | **40** |

**Why:**

- **The reverse of E16, for the unspent part.** The money moves from one
  liability back to the other. The platform still owes it to Hana; it is
  spendable online again.
- **Escrow ends at zero.** `2050:vch-0001` received 100 (E16), paid 60 (E17)
  and returned 40 (E18). A voucher that does not end at zero after expiry is
  a sign of a bug or an unsettled receipt.
- **The voucher is closed for good.** After the return, the device's
  allowance is gone, so any further receipt against `vch-0001` cannot be
  settled from escrow.

**Balances after E01–E18:**

| Account | Balance |
|---|---:|
| 1010 Bank: customer funds account | 1,402.20 Dr |
| 2010:cust-abebe Customer wallet | 720.00 Cr |
| 2010:cust-hana Customer wallet | 620.00 Cr |
| 2030:merch-coffee Merchant balance | 58.80 Cr |
| 2020, 2040, 2050 (all pending and escrow accounts) | 0.00 |
| 4010 Payment fee revenue | 3.40 Cr |

**The safeguarding check:** the customer funds account holds 1,402.20 ETB. The
platform owes customers 1,340.00 and merchants 58.80, a total of 1,398.80.
The difference, 3.40 ETB, is exactly the fee revenue earned and not yet swept
to the operating account (see E07). Every birr owed is backed by a birr in the
bank. The full trial balance is in [trial-balance.md](../trial-balance.md).

**Open questions:**

- **Late receipts.** What if a receipt for a payment made *before* expiry
  arrives *after* the 40 ETB has been returned? The merchant was paid in good
  faith, but escrow is empty. The architecture sends it to suspense under a
  written loss policy: who absorbs it, the platform, the merchant or Hana?
- **Grace period.** Should the return wait a few days after expiry, to give
  late receipts a chance to arrive before the escrow is released?
- **Lost or cloned devices.** If Hana reports the phone lost, the voucher can
  be revoked early. If the same voucher shows two different receipts with the
  same counter, the device is frozen (spec §5.5, `CONFLICT`). How are those
  cases booked?
