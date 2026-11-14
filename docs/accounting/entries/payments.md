# Payments (E06–E10)

[All entries](README.md) · [Chart of accounts](../chart-of-accounts.md)

Payments move money between balances the platform already holds. No cash
enters or leaves the bank, so the bank account (1010) is never touched in this
file. That is the key difference from deposits and withdrawals.

The examples continue from E05: Abebe's wallet holds 1,000 ETB. Hana is a
second customer with an empty wallet, and Merkato Coffee is a merchant.

## E06. P2P transfer

**Scenario:** Abebe sends 150 ETB to Hana from the wallet app. Both are
customers of the platform. There is no fee.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-abebe Customer wallet | Liability | 150 | |
| 2 | 2010:cust-hana Customer wallet | Liability | | 150 |
| | **Total** | | **150** | **150** |

**Why:**

- **Line 1, debit Abebe's wallet (liability ↓).** The platform now owes Abebe
  150 ETB less.
- **Line 2, credit Hana's wallet (liability ↑).** The platform now owes Hana
  150 ETB more.
- **The equation stays in balance:** Assets 0 = Liabilities −150 + 150. The
  platform owes exactly the same total as before; only *who* it owes has
  changed. The cash in the bank does not move, because both customers' money
  sits in the same customer funds account.

**Open questions:**

- Should a P2P transfer to someone without a wallet be possible (for example,
  to a phone number that registers later)? If so, the money needs a holding
  account until the recipient claims it, and a rule for returning it if they
  never do.
- Are there P2P fees in some cases (for example, above a free monthly limit)?
  If so, E06 gains a third line, like E07.

## E07. QR merchant payment with fee

**Scenario:** Abebe scans Merkato Coffee's QR code and pays 100 ETB. The
merchant has agreed to a 2% fee, which the platform keeps. The customer pays
100, and the merchant receives 98.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-abebe Customer wallet | Liability | 100 | |
| 2 | 2030:merch-coffee Merchant balance | Liability | | 98 |
| 3 | 4010 Payment fee revenue | Revenue | | 2 |
| | **Total** | | **100** | **100** |

**Why:**

- **Line 1, debit Abebe's wallet (liability ↓).** Abebe pays the full price,
  so the platform owes Abebe 100 ETB less.
- **Line 2, credit the merchant balance (liability ↑).** The platform now owes
  Merkato Coffee 98 ETB, which it will pay out in a settlement (E15). Merchant
  balances are kept in their own control account (2030), separate from
  customer wallets, because merchants are paid out differently and have
  different limits.
- **Line 3, credit fee revenue (revenue ↑).** The 2 ETB is no longer owed to
  anyone. The platform has *earned* it, so it is revenue, and revenue
  increases on the credit side.
- **The equation stays in balance:** Assets 0 = Liabilities −100 + 98 +
  Revenue 2. This is the first entry where the platform earns money: 2 ETB of
  debt turns into 2 ETB of income.

**One entry, three lines.** The payment and the fee are booked together as a
single balanced entry, not as a payment followed by a separate fee charge.
Either all three lines post or none do, so a customer can never be charged a
fee for a payment that failed. This is the reason Mezgeb uses multi-line
journal entries ([architecture §4](../../architecture.md#4-data-model-at-a-glance)).

**Where the fee money sits.** The 2 ETB is still physically in the customer
funds bank account (1010). From now on, that account holds slightly more than
the platform owes its customers: the difference is earned fees. See the
balance check after E18.

**Template:** `merchant_payment.v1` with `channel = qr` and
`merchant_fee = 2` ([ADR-0014](../../adr/0014-payment-channels-and-fees.md)).

**Decided in step 2.03:**

- **Rounding is the client's.** The client applies its fee schedule and its
  rounding rule, and sends the fee in santim. The ledger never rounds; it
  checks only that the fees are at most the price and in its currency.
- **A payer-paid fee is possible.** With `payer_fee`, the customer pays the
  fee on top of the price: for a 100 ETB price and a 1 ETB payer fee, the
  wallet is debited 101, the merchant credited 100 and fee revenue 1. Both
  kinds of fee can apply to one payment; fee revenue gets both in one line.

**Open questions:**

- How often is earned fee money moved from the customer funds account to the
  platform's operating account (a "fee sweep")?

## E08. Card tap payment

**Scenario:** Abebe taps a contactless card linked to the wallet on Merkato
Coffee's terminal and pays 200 ETB. The terminal is online, and the payment is
approved immediately. The fee for card payments is 1.5%.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-abebe Customer wallet | Liability | 200 | |
| 2 | 2030:merch-coffee Merchant balance | Liability | | 197 |
| 3 | 4010 Payment fee revenue | Revenue | | 3 |
| | **Total** | | **200** | **200** |

**Why:** the lines are the same as in E07, with different amounts and a
different fee rate. The reasoning for each line does not change.

**The payment channel is not an accounting fact.** A card tap, a QR scan and a
fingerprint (E09) are different ways for the customer to *authorise* a
payment. Once authorised, the money moves the same way. The channel is stored
as metadata on the entry (for reports and disputes), not as different
accounts. This keeps one posting template, `merchant_payment.v1`, for all
three, with `channel = card_tap` here. A phone tap books the same way, with
`channel = phone_tap` ([ADR-0014](../../adr/0014-payment-channels-and-fees.md)).

**Open questions:**

- Some card payments are *authorised* first and *captured* later (for example,
  when the final amount is not known yet). That needs a hold on the wallet,
  then a capture or a release. Holds are modelled in step 1.14; does any
  supported terminal need them?
- An offline card tap, when the terminal has no connection, is a different
  flow. It is booked against offline escrow (E16–E18), not as E08.

## E09. Fingerprint payment

**Scenario:** Hana pays 50 ETB at Merkato Coffee by fingerprint. The fee is 2%,
the same as for QR payments.

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-hana Customer wallet | Liability | 50 | |
| 2 | 2030:merch-coffee Merchant balance | Liability | | 49 |
| 3 | 4010 Payment fee revenue | Revenue | | 1 |
| | **Total** | | **50** | **50** |

**Why:** the same as E07 and E08. Hana's wallet goes down by the full price,
the merchant is owed the price minus the fee, and the platform earns the fee.
Hana's wallet drops from 150 to 100 ETB.

**Open questions:**

- Biometric payments have a higher risk of disputed authorisation ("I didn't
  pay that"). Should they have a lower per-payment limit, or a short hold
  before the merchant's balance can be settled?

## E10. Payment request paid

**Scenario:** Hana pays for a shared lunch and sends Abebe a payment request
for 80 ETB. Abebe accepts the request the next day and pays it.

**Step 1: Hana creates the request.** No journal entry. A request is a
message, not a movement of money: nobody's balance changes, and the platform
does not owe anyone anything new. It is stored by the client application, not
in the ledger.

**Step 2: Abebe pays the request.** This is booked like a P2P transfer (E06):

| # | Account | Type | Dr | Cr |
|---|---|---|---:|---:|
| 1 | 2010:cust-abebe Customer wallet | Liability | 80 | |
| 2 | 2010:cust-hana Customer wallet | Liability | | 80 |
| | **Total** | | **80** | **80** |

**Why:**

- **Only real money movements are journal entries.** Requests, invoices and
  quotes become accounting events only when they are paid. Booking them
  earlier would show money owed between customers, which the platform does
  not guarantee.
- **The lines are the same as E06:** the platform owes Abebe 80 ETB less and
  Hana 80 ETB more. The entry references the request ID, so the request can
  be shown as "paid".

**Template:** `payment_request_settle.v1`, which requires the
`payment_request` it pays. Unlike `p2p_transfer.v1`, the payee can also be
a merchant (an invoice), and the same fees as a merchant payment can apply
([ADR-0014](../../adr/0014-payment-channels-and-fees.md)).

**Balances after E01–E10:**

| Account | Balance |
|---|---:|
| 1010 Bank: customer funds account | 1,000 Dr |
| 2010:cust-abebe Customer wallet | 470 Cr |
| 2010:cust-hana Customer wallet | 180 Cr |
| 2030:merch-coffee Merchant balance | 344 Cr |
| 4010 Payment fee revenue | 6 Cr |

The bank still holds 1,000 ETB: 994 owed to customers and the merchant, plus
6 ETB of earned fees.

**Open questions:**

- Should a request be able to *reserve* money on the payer's side before it is
  accepted? If so, it becomes a hold (step 1.14), not just a message.
- What happens if Abebe pays only part of a request? Is that allowed, and does
  the request stay open for the rest?
