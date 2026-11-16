# Offline Scenario Walkthroughs

[Documentation](../README.md) · Protocol: [protocol.md](protocol.md) · Threats: [threat-addendum.md](threat-addendum.md) · Entries: [offline-escrow.md](../accounting/entries/offline-escrow.md)

The ten adversarial scenarios of step 2.10, walked on paper against
[protocol v1](protocol.md). Each scenario gives:
- what happens, step by step;
- the outcome of every receipt (§9);
- the accounting result;
- the invariants that hold.

**Common setup.**

| Who | Device | Tier | Voucher | Escrow | First counter |
|---|---|---|---|---:|---:|
| Hana (payer) | Phone `dev-hana` | B | `vch-H` | 100 ETB | 1 |
| Abebe (payer) | Card `dev-abebe` | A | `vch-A` | 500 ETB | 1 |

Merchants: Coffee (`merch-coffee`) and Stall (`merch-stall`), each with a
terminal. The merchant fee is 2% (E07).

## 1. Clone

**A copy of Hana's phone spends the same allowance.**

1. Hana's phone is cloned, key and app state included. At that moment its
   counter is 3 and `remaining` is 60.
2. The real phone pays Coffee 50 ETB, signing counter 4.
3. The clone pays Stall 45 ETB, also signing counter 4, with the same
   `prev_digest`.
4. Coffee uploads first.

| Receipt | Outcome | Entry |
|---|---|---|
| Counter 4, Coffee, 50 | `SETTLED` | E17 (from escrow) |
| Counter 4, Stall, 45 | `CONFLICT`: the device and voucher freeze | none |
| Any later receipt of either copy | `FROZEN`, kept for the dispute | none |

**The dispute** finds both receipts validly signed by Hana's key.
- Stall's 45 ETB is paid as a late receipt, from Hana's wallet (E31).
- The escrow left, 10 ETB, returns to Hana's wallet (E27).
- The device is revoked.

**Accounting.** Coffee is paid 50 from escrow and Stall 45 from Hana's
wallet. Escrow ends at 0. The platform loses nothing while Hana's wallet
can pay; otherwise the gap goes to 5030 (E29).

**Invariants.** INV-21 holds: one counter settled once from escrow, the
second raised a conflict and froze the device. INV-19 holds: escrow paid 50
of 100.

**A card instead of the phone:** its key cannot be copied (H1), so this
scenario needs the chip itself broken (OT-01).

## 2. Replay

**Coffee uploads Hana's receipt three times, and sends a copy to Stall.**

1. Hana pays Coffee 30 ETB, counter 1.
2. Coffee's terminal uploads it, then uploads it twice more.
3. Coffee also gives a copy to Stall, which uploads it too.

| Upload | Outcome | Entry |
|---|---|---|
| Coffee, first | `SETTLED` | E17, to Coffee |
| Coffee, second and third | `DUPLICATE` | none |
| Stall's copy | `DUPLICATE`: the same digest, and its payee is Coffee | none |

**Accounting.** One entry: Coffee is paid 29.40, and the fee is 0.60.
Stall gains nothing, because `payee_account_id` is inside the signed body.

**Invariants.** INV-20: a `(device, counter)` settles at most once.

## 3. Forged amount

**Stall changes a 20 ETB receipt to 200 ETB before uploading it.**

1. Hana pays Stall 20 ETB, counter 2, after counter 1 settled.
2. Stall edits the amount to 200 and uploads the receipt with Hana's
   original signature.

| Receipt | Outcome | Entry |
|---|---|---|
| The edited receipt (200) | `BAD_SIGNATURE`: the signature covers the body, amount included | none |
| The original, if Stall uploads it later | `SETTLED` | E17, for 20 |

**Accounting.** None for the forgery. The audit log records Stall's refused
upload (ADR-0018).

**Invariants.** INV-22: only receipts with a valid device signature settle.

## 4. Clock rollback

**Hana sets the phone's clock back to spend after the voucher expired.**

The voucher's `expires_at` is day 3 and its grace runs to day 10. The day-3
receipts are all uploaded by day 4.

1. On day 5, Hana's clock reads day 2, and her phone pays Coffee 40 ETB
   (counter 3). Its `event_time` says day 2.
2. Coffee uploads on day 6.

| Receipt | Outcome | Entry |
|---|---|---|
| Counter 3, 40 | `SETTLED`: the voucher is in `GRACE` by **ledger** time, and the escrow has 40 | E17 |

**The `event_time` check.** Day 2 lies between the voucher's issue and now,
so it is recorded. Had it been outside that range, it would be left empty
and kept only in metadata.

**Had Coffee uploaded on day 12**, after the return, the outcome would be
`SETTLED_LATE`: E28, from Hana's wallet.

**Accounting.** Escrow or wallet, never more than Hana had. The clock
changed only a recorded time, not the money.

**Invariants.** INV-19: no more than the escrow from escrow. Order comes
from the counter, never the clock.

## 5. Payee double-upload

**Coffee's terminal uploads a batch, times out, and uploads it again; then
Hana's phone uploads the same receipts.**

1. Hana pays Coffee twice: counters 1 and 2, 10 ETB each.
2. Coffee uploads `[1, 2]`. The response is lost, so it uploads `[1, 2]`
   again.
3. A day later, Hana's phone syncs and uploads `[1, 2]`.

| Upload | Outcome | Entry |
|---|---|---|
| Coffee's first batch | `SETTLED`, `SETTLED` | E17 × 2 |
| Coffee's retry | `DUPLICATE`, `DUPLICATE` | none |
| Hana's phone | `DUPLICATE`, `DUPLICATE` | none |

**Accounting.** Two entries, of 10 ETB each: Coffee gets 9.80 for each, and
the fee is 0.20 each.

**Invariants.** INV-20, and spec §5.5: the result is the same whoever
uploads, and however often.

## 6. Payer never syncs

**Abebe's card pays all week and never connects; only the merchants
upload.**

1. Abebe's card pays Coffee 120 ETB (counter 1) and Stall 80 ETB
   (counter 2). Cards cannot upload.
2. Stall uploads on day 2, Coffee on day 4.

| Upload | Outcome | Entry |
|---|---|---|
| Stall, counter 2 (day 2) | `WAITING_FOR_GAP`: counter 1 has not arrived | none yet |
| Coffee, counter 1 (day 4) | `SETTLED`; then counter 2 settles in turn | E17, E17 |

**The voucher ends** on day 7 (expiry), then grace to day 14. On day 14,
the 300 ETB left returns to Abebe's wallet (E18).

**Accounting.** Escrow 500 = 120 + 80 + 300 returned, ending at 0.

**Invariants.** INV-19 and INV-23: escrow is zero after the return. Abebe
syncing never mattered.

## 7. Lost device

**Hana loses her phone; a finder spends from it.**

1. Hana's voucher has 70 ETB left.
2. On day 1, Hana reports the phone `LOST` through the client. Its
   certificate is revoked, and no new voucher can be issued for it.
3. On day 2, the finder pays Stall 30 ETB (counter 4). Stall's terminal has
   not refreshed its revocation list, so it accepts the receipt.
4. On day 2, the finder also tries to pay 60 ETB: the phone's own
   `remaining` is 40, so it refuses.
5. Stall uploads.

| Receipt | Outcome | Entry |
|---|---|---|
| Counter 4, Stall, 30 | `SETTLED`: the voucher is still `ACTIVE`, and the receipt valid | E17 |

**The voucher ends:** it expires, grace passes, and the 40 ETB left returns
to Hana (E18).

**Accounting.** Hana lost at most the 70 ETB of escrow; the finder spent 30
of it. Stall, which acted in good faith, is paid.

**Invariants.** INV-19: the loss is bounded by the escrow.

**A card in Hana's place:** above the PIN threshold, the finder cannot pay
(H6).

## 8. Partial sync

**Hana's phone uploads half its receipts, then loses its connection.**

1. Hana made receipts 1 to 6 of 15 ETB each: 90 ETB of 100.
2. The phone uploads 1 to 3, then drops.
3. Coffee, the payee of 5, uploads 5 the next day.
4. Stall, the payee of 4 and 6, uploads them three days later.

| Receipt | Outcome | Entry |
|---|---|---|
| 1, 2, 3 | `SETTLED` | E17 × 3 |
| 5 (day 1) | `WAITING_FOR_GAP`, behind 4 | none yet |
| 4 (day 3) | `SETTLED`; then 5 settles in turn | E17, E17 |
| 6 (day 3) | `SETTLED` | E17 |

**Had Stall never uploaded receipt 4:**
- receipt 5 would settle after the 72-hour gap timeout, marked
  `gap_skipped`;
- receipt 4 arriving later would have to match receipt 5's `prev_digest`;
- it would then settle from the escrow left, or under the loss policy.

**Accounting.** Six entries of 15 ETB each. The 10 ETB left returns at the
end (E18).

**Invariants.** INV-19, INV-20, INV-22: the chain is checked wherever
receipts arrive in order.

## 9. Voucher expiry race

**A receipt is made just before expiry and arrives after the return.**

1. Abebe's card pays Stall 90 ETB, one minute before `expires_at` (ledger
   time).
2. Stall's terminal breaks, and is repaired 20 days later. By then the
   7-day grace has passed, and the escrow's 300 ETB went back to Abebe on
   day 14.
3. Stall uploads on day 27.

| Receipt | Outcome | Entry |
|---|---|---|
| 90 ETB to Stall | `SETTLED_LATE`: the voucher is `RETURNED` and the receipt valid | E28 if Abebe's wallet has 90; E29 if it has less |

**If Abebe's wallet holds 25 ETB:**
- 25 ETB comes from the wallet and 65 from 5030 (E29);
- the 65 ETB shortfall is recorded against Abebe;
- Abebe gets no new voucher until he repays it (E30).

**Had Stall uploaded after the late deadline** (90 days past the return),
the outcome would be `EXPIRED`: refused, with no entry, and Stall bears it.

**Accounting.** The merchant is paid; the payer pays first; the platform
loses only what the payer cannot pay, and only until the payer repays.

**Invariants.** INV-23: the escrow was zero after its return. The late
payment never touches it.

## 10. Colluding merchant

**Hana and Stall try to make the platform pay for money that does not
exist.**

1. Hana pays Stall 100 ETB offline, counter 1. This is the whole voucher.
2. Stall holds the receipt.
3. After the grace period, the escrow's 100 ETB returns to Hana's wallet
   (E18).
4. Online, Hana spends her whole wallet, leaving it at 0.
5. Stall then uploads the receipt, within the late deadline.

| Receipt | Outcome | Entry |
|---|---|---|
| 100 ETB to Stall | `SETTLED_LATE`: the wallet has 0 | E29: 0 from the wallet, 100 from 5030 |

**The consequences:**
- A 100 ETB shortfall is recorded against Hana.
- Hana gets no new voucher on any device until she repays it.
- Late-upload ratios flag Stall in monitoring (step 2.15).

**Repeating it is capped:**
- Hana: one voucher per device, and the per-customer cap of 5,000 ETB.
  After the first shortfall, no new vouchers.
- Stall: a pattern of late uploads.

**The other variants fail:**
- Uploading the receipt twice: `DUPLICATE`.
- Inventing receipts: `BAD_SIGNATURE`.
- Hana signing receipts beyond the voucher: `OVER_LIMIT`, which freezes the
  device, with the excess on Hana.

**Accounting.** The platform loses at most what Hana had in escrow, once
(5030), and keeps a claim against her (E30 if repaid).

**Invariants.** INV-19 holds. The collusion is bounded and detected, not
prevented. No protocol can stop two parties from agreeing to cheat (OT-13).

## Summary

Every scenario ends in a defined outcome and a defined accounting result:

| # | Scenario | Outcomes | Entries | Platform loss |
|---|---|---|---|---|
| 1 | Clone | `SETTLED`, `CONFLICT`, `FROZEN` | E17, E31, E27 | None while the payer's wallet pays |
| 2 | Replay | `SETTLED`, `DUPLICATE` | E17 | None |
| 3 | Forged amount | `BAD_SIGNATURE` | none | None |
| 4 | Clock rollback | `SETTLED` (or `SETTLED_LATE`) | E17 (or E28) | None |
| 5 | Payee double-upload | `SETTLED`, `DUPLICATE` | E17 | None |
| 6 | Payer never syncs | `WAITING_FOR_GAP`, `SETTLED` | E17, E18 | None |
| 7 | Lost device | `SETTLED` | E17, E18 | None; the payer loses at most the escrow |
| 8 | Partial sync | `SETTLED`, `WAITING_FOR_GAP` | E17, E18 | None |
| 9 | Voucher expiry race | `SETTLED_LATE` (or `EXPIRED`) | E28 or E29, E30 | Only what the payer cannot pay, until repaid |
| 10 | Colluding merchant | `SETTLED_LATE` | E29 | Up to one voucher per customer, then blocked |
