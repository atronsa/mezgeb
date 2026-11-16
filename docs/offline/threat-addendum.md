# Offline Threat Model Addendum

[Documentation](../README.md) · Protocol: [protocol.md](protocol.md) · Main model: [threat-model.md](../threat-model.md) (TM-17 to TM-23) · Scenarios: [scenarios.md](scenarios.md)

The offline threats in detail, each desk-checked against
[protocol v1](protocol.md). Offline, the ledger cannot stop a payment as it
happens. Every control here is therefore one of two kinds:

- **Prevent:** the payment cannot be made, or cannot be settled.
- **Bound and detect:** the loss is capped and the wrongdoing shows up.

## Attackers

| Attacker | Has | Wants |
|---|---|---|
| **Payer** | Their own device, its key (in hardware), physical access, time | Spend more than their allowance; deny payments they made |
| **Payee** (merchant) | Receipts, a terminal, its key | Be paid twice; be paid more; be paid for receipts never given |
| **Payer and payee together** | Both | Make the platform pay for money that never existed |
| **Thief** | A lost or stolen device | Spend its allowance |
| **Outsider** | Radio range at a tap; uploads | Forge, replay or block payments |
| **Insider** | Database or service access | Change offline records after the fact |

## Desk check

**Result** is what the desk check of the protocol found:

- **Prevented:** the attack fails.
- **Bounded:** it can succeed, but the loss is capped by the named limit.
- **Detected:** it shows up as a conflict, an alert or a finding.

| ID | Threat | STRIDE | Main TM | Protocol controls | Result |
|---|---|---|---|---|---|
| OT-01 | Clone a **card's** key to spend twice | S, T | TM-17 | Key generated in the secure element and never exportable (H1); counter and signing as one step (H3) | Prevented, as far as the chip holds (vendor confirmation needed, §15) |
| OT-02 | Clone or restore a **phone** to spend twice | S, T | TM-17 | No counter bound to the key: the same counter signed twice is a `CONFLICT` (§10), and the device freezes. The escrow pays only the first; the rest is paid by the payer's wallet first (§12) | Bounded by the voucher (tier B 1,000, C 200 ETB) and the per-customer cap; detected |
| OT-03 | Replay a receipt: upload it again, or to another payee | T | TM-17 | Settled once per `(device_id, counter)` (INV-20), with the same digest; `payee_account_id` is inside the signed body, so it pays only that payee | Prevented: `DUPLICATE` |
| OT-04 | Forge a receipt, or change its amount or payee | S, T | TM-18, TM-23 | ECDSA over the canonical body (§3, §8); checked by the payee offline and by the ledger | Prevented: `BAD_SIGNATURE` |
| OT-05 | Alter a valid signature into another valid one | T | TM-18 | Low-S only (§2), and the digest never includes the signature (§3), so a second signature of one body is the same receipt | Prevented |
| OT-06 | Roll the device clock back or forward to use an expired voucher, or to backdate | T | TM-19 | Expiry and order are decided by ledger time and the counter, never `event_time` (§7, §9). A receipt made after expiry settles from escrow during grace, or under the loss policy after it | Bounded by the escrow; the claimed time is only recorded |
| OT-07 | Spend more than the voucher with a tampered app | T, E | TM-17 | The card refuses to sign above `remaining` (H4). Above the escrow left, the receipt is `OVER_LIMIT`: the device freezes and the excess goes to the loss policy (§10, §12) | Bounded by the escrow (INV-19), plus the payer's own wallet; detected |
| OT-08 | Forge a voucher to fake an allowance | S | TM-18 | Vouchers are signed by an issuer key certified by the root; payees verify it offline (§8). The ledger settles only vouchers it issued (`UNKNOWN_VOUCHER`) | Prevented for any payee that checks; a payee that skips the check bears the loss |
| OT-09 | Steal an issuer key | S, E | TM-14 | The root key is offline; issuer keys rotate (90 days) and can be revoked (§4). Even a validly signed forged voucher does not settle, because it is not in the ledger | Bounded: payees accepting forged vouchers lose until revocation reaches them |
| OT-10 | Pay offline, then never sync, to avoid being charged | R | TM-21 | The payee uploads; the payer's device is not needed (§9) | Prevented |
| OT-11 | Pay offline, then report the device lost to get the escrow back | R | TM-20 | `LOST` does not close the voucher: it expires and returns normally, after the grace period (§13) | Prevented while receipts arrive within grace; afterwards the loss policy charges the payer |
| OT-12 | Hold receipts, then upload them late, hoping the platform pays | R, T | TM-21 | The loss policy pays from the **payer's** wallet first (§12). The 90-day late deadline (`EXPIRED`) and late-upload ratios per merchant are monitored (step 2.15) | Bounded by the voucher and the deadline |
| OT-13 | Payer and payee together: spend offline, empty the wallet online after the return, upload late so the platform pays | T | TM-10 | The shortfall is recorded against the payer, and no new voucher is issued until it is repaid (§12). The loss is capped by one voucher per device and the per-customer cap. Late-receipt losses are tracked per payer and payee | Bounded: at most one voucher's worth per customer, then blocked |
| OT-14 | Withhold one receipt to block a payee's later ones | D | — | The gap timeout settles waiting receipts after 72 hours, or at the return (§11) | Bounded: a delay of at most 72 hours |
| OT-15 | Freeze someone else's device by inventing a conflicting receipt | D | — | A conflicting receipt must be signed with the device's key (§10) | Prevented |
| OT-16 | Register a phone or software key as a tier A card, for higher limits | S, E | TM-17 | The tier comes from attestation chained to roots the client trusts (§5). No attestation means tier C | Prevented, as far as the attestation roots hold |
| OT-17 | Flood the upload endpoint | D | TM-03 | Rate limits per client (step 2.08); a batch size limit; each receipt processed alone | Bounded |
| OT-18 | Change offline records in the database to hide a theft | T | TM-11 | Settlements are journal entries, hash-chained and signed (ADR-0018); receipt records and state changes are audit events | Detected by `verify --chain` |
| OT-19 | Steal a device and spend its allowance | S | TM-20 | PIN above a small amount (H6); spending bounded by the remaining escrow; no new voucher once reported lost (§13) | Bounded by the escrow left |
| OT-20 | Payer and payee upload the same receipt, in any order | — | — | Idempotent by `(device_id, counter)` and digest (§9) | Prevented: one `SETTLED`, the other `DUPLICATE` |

## What stays open

- **Phone counters** (OT-02). Without a hardware counter bound to the key,
  a phone clone is detected only after the fact. The tier limits are the
  real control.
- **Chip claims** (OT-01, H1–H8). Until a vendor confirms them for a
  specific chip, tier A rests on requirements, not on tested hardware.
- **Collusion** (OT-13) cannot be prevented by any protocol. It is bounded
  by the limits and the shortfall block, and detected by monitoring
  (step 2.15).
- **This addendum and the protocol await an external security review**
  ([review-request.md](review-request.md)).
