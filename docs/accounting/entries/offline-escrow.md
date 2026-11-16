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

## Every offline state (step 2.10)

E16–E18 are the normal life of a voucher. The entries below book every
other state the [offline protocol](../../offline/protocol.md) defines
(ADR-0019). Each is its own small example, on its own voucher, with the 2%
merchant fee of E07. They are not part of the running balances above.

| State | Entry |
|---|---|
| Voucher issued | E16 |
| Receipt settled from escrow, in order or after a skipped gap | E17 (a skipped gap changes only the receipt's metadata) |
| Voucher returned after expiry and grace | E18 |
| Voucher closed early, online | E25 |
| A receipt over the escrow left | E26 |
| A frozen voucher's escrow returned after its dispute | E27 |
| A late receipt, the payer's wallet pays | E28 |
| A late receipt, the wallet pays part and the loss account the rest | E29 |
| The payer repays a shortfall | E30 |
| A cloned device's second receipt, paid after the dispute | E31 |

**States that post nothing,** leaving every balance as it was:

- **The receipt waits or is kept:** `DUPLICATE`, `WAITING_FOR_GAP`,
  `FROZEN`.
- **It freezes the device:** `CONFLICT`.
- **It is refused:** `BAD_FORMAT`, `UNKNOWN_DEVICE`, `BAD_SIGNATURE`,
  `UNKNOWN_VOUCHER`.
- **It is refused after the late deadline,** so the payee bears it:
  `EXPIRED`.

## E25. Voucher closed early

**Scenario:** Abebe's card has voucher `vch-0002` for 500 ETB.
1. Back in town, Abebe taps the card on the phone app.
2. The app uploads the card's two receipts, 120 and 80 ETB, which settle as
   E17.
3. It uploads a signed close with the last counter.
4. The ledger returns what is left.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2050:vch-0002 Offline escrow | Liability | 300.00 | |
| 2 | 2010:cust-abebe Customer wallet | Liability | | 300.00 |
| | **Total** | | **300.00** | **300.00** |

**Why:**

- **The same booking as E18, earlier.**
- **The close carries the card's last counter and last receipt,** so the
  ledger knows nothing else can be spent from this voucher. That makes it
  safe to release the escrow before expiry.
- **Escrow ends at zero:** 500 − 120 − 80 − 300.

## E26. A receipt over the escrow left

**Scenario:** Hana's phone (tier B) had `vch-0003` for 100 ETB.
1. Its first receipt, 70 ETB to Merkato Coffee, settled (E17), leaving
   30 ETB.
2. A second receipt arrives, for 50 ETB to the same merchant. A correct
   device would have refused to sign it, so the device froze
   (`OVER_LIMIT`).
3. The dispute finds the phone was restored from an old backup.
4. It is resolved under the loss policy: the escrow pays the 30 ETB left,
   and Hana's wallet (580 ETB) pays the other 20.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2050:vch-0003 Offline escrow | Liability | 30.00 | |
| 2 | 2010:cust-hana Customer wallet | Liability | 20.00 | |
| 3 | 2030:merch-coffee Merchant balance | Liability | | 49.00 |
| 4 | 4010 Payment fee revenue | Revenue | | 1.00 |
| | **Total** | | **50.00** | **50.00** |

**Why:**

- **The merchant accepted a validly signed receipt,** so it is paid in full,
  minus the usual fee.
- **The escrow pays what it can.** The payer, whose device signed more than
  its voucher, pays the rest (ADR-0019).
- **The escrow is now zero,** so the voucher has nothing left to return.

## E27. A frozen voucher's escrow returned after its dispute

**Scenario:** Sara's phone voucher `vch-0004` (200 ETB) froze on a conflict,
after one receipt of 50 ETB had settled. The dispute (E31) is resolved, and
the 150 ETB left in escrow goes back to Sara's wallet.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2050:vch-0004 Offline escrow | Liability | 150.00 | |
| 2 | 2010:cust-sara Customer wallet | Liability | | 150.00 |
| | **Total** | | **150.00** | **150.00** |

**Why:**

- **A frozen voucher holds its escrow until the dispute ends,** so the money
  cannot be released while the facts are unknown.
- **Once the dispute is resolved,** the escrow returns as in E18, and ends at
  zero.

## E28. A late receipt paid from the payer's wallet

**Scenario:** Hana's `vch-0005` expired, its grace period passed, and its
100 ETB went back to her wallet (E18). Six days later, a market stall's
terminal finally uploads a receipt dated before expiry, for 40 ETB. The
receipt is valid, and Hana's wallet holds 600 ETB.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-hana Customer wallet | Liability | 40.00 | |
| 2 | 2030:merch-stall Merchant balance | Liability | | 39.20 |
| 3 | 4010 Payment fee revenue | Revenue | | 0.80 |
| | **Total** | | **40.00** | **40.00** |

**Why:**

- **The escrow was returned to the wallet,** so the money the receipt spent
  is back in the wallet.
- **Paying from it puts Hana exactly where she would have been** had the
  receipt arrived in time.
- **The merchant is paid in full.**

## E29. A late receipt, partly from the loss account

**Scenario:** As in E28, but the late receipt is for 90 ETB and Hana's
wallet holds only 25 ETB.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-hana Customer wallet | Liability | 25.00 | |
| 2 | 5030 Offline settlement losses | Expense | 65.00 | |
| 3 | 2030:merch-stall Merchant balance | Liability | | 88.20 |
| 4 | 4010 Payment fee revenue | Revenue | | 1.80 |
| | **Total** | | **90.00** | **90.00** |

**Why:**

- **The wallet pays what it can,** and cannot go negative (INV-8). The
  platform's loss account pays the rest, so the merchant is still paid.
- **5030 is an expense:** the platform has lost 65 ETB, unless Hana repays
  it.
- **The shortfall is recorded against Hana.** No device of hers gets a new
  voucher until it is repaid (E30).

## E30. The payer repays a shortfall

**Scenario:** Hana tops up her wallet and repays the 65 ETB shortfall of
E29.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-hana Customer wallet | Liability | 65.00 | |
| 2 | 5030 Offline settlement losses | Expense | | 65.00 |
| | **Total** | | **65.00** | **65.00** |

**Why:**

- **Crediting the loss account reverses the loss:** the platform is made
  whole.
- **Hana's offline payments are allowed again.**

## E31. A cloned device's second receipt

**Scenario:** Sara's phone was cloned.
1. Both copies signed counter 7 against `vch-0004`: one for 50 ETB to
   Merkato Coffee, the other for 60 ETB to a stall.
2. The coffee receipt arrived first and settled (E17).
3. The stall's arrived second: `CONFLICT`, and the device froze.
4. The dispute finds both validly signed by Sara's device key.
5. Under the loss policy, the stall is paid too, from Sara's wallet
   (300 ETB).

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-sara Customer wallet | Liability | 60.00 | |
| 2 | 2030:merch-stall Merchant balance | Liability | | 58.80 |
| 3 | 4010 Payment fee revenue | Revenue | | 1.20 |
| | **Total** | | **60.00** | **60.00** |

**Why:**

- **Escrow never settles one counter twice** (INV-21): it paid only the
  first receipt.
- **The second is paid as a late receipt would be:** from the payer's wallet
  first, then from 5030 if the wallet were short (as in E29).
- **The payer is liable for what her device signed,** as for a card used by
  someone who copied it. The device is revoked, and the client follows up
  with Sara.
- **Then the voucher's escrow returns** (E27).
