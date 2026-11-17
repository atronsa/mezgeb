# Offline Payment Protocol v1

[Documentation](../README.md) · Spec: [§5](../spec.md#5-offline-sync) · Decisions: [ADR-0010](../adr/0010-offline-escrow-model.md), [ADR-0019](../adr/0019-offline-protocol-v1.md) · Threats: [threat-addendum.md](threat-addendum.md) · Scenarios: [scenarios.md](scenarios.md) · Entries: [E16–E18, E25–E31](../accounting/entries/offline-escrow.md)

**Status: v1 (step 2.10).** Built: devices and vouchers (step 2.11),
receipts and their settlement (step 2.12), gap timeouts, expiry, late
receipts, shortfalls and disputes (step 2.13), and the deterministic
simulation that runs fleets of devices against all of it (step 2.14,
`mezgeb-sim --workload offline`). Not built yet: the early close (§7.1) and
payee acknowledgements (§8). Test
vectors: [test-vectors/offline](../../test-vectors/offline/README.md).
Pending external security review ([review-request.md](review-request.md)).

A payer's device with no connection pays a payee by signing a **receipt**
against a **voucher**: an allowance the ledger reserved in escrow while the
device was online. The receipt is a claim, not a payment. When it reaches
the ledger, from the payer or the payee, in any order and any number of
times, the ledger checks it and settles it from escrow **exactly once**.

## 1. Parties

| Party | What it is | Holds |
|---|---|---|
| **Ledger** | Mezgeb's service | The issuer keys; device, voucher and receipt records; the escrow accounts |
| **Client** | The platform integrating with the ledger (ADR-0017) | Its app, its terminals, its customers' wallets |
| **Payer device** | A smart card (tier A) or a phone (tiers B, C) | A device key in secure hardware; its current voucher; its counter |
| **Payee device** | A merchant terminal or a phone | Its own device key (for acknowledgements); the receipts it received |

## 2. Cryptography

- **Signatures:** ECDSA over P-256 (secp256r1) with SHA-256 (FIPS 186-5).
  - A signature is 64 bytes: `r ‖ s`, each 32 bytes big-endian.
  - `s` must be at most `n/2` (low-S). A verifier refuses any other, so a
    valid signature cannot be altered into a second valid one.
  - Deterministic nonces (RFC 6979) are recommended, and required where the
    hardware allows them.
- **Public keys:** SEC1 compressed points, 33 bytes.
- **Key ID:** the first 8 bytes of the SHA-256 of the compressed key, in
  hex.
- **Hashes:** SHA-256.
- **The ledger's own receipts and checkpoints** stay Ed25519 (ADR-0018). They
  are a different thing from the offline receipts here, which are signed by
  devices.

## 3. Canonical encoding

Every signed message is encoded byte for byte as follows; nothing is JSON.
These are the rules of the ledger's chains (`mezgeb-core::chain`).

| Kind | Encoding |
|---|---|
| Integers | Big-endian: `u8`, `u32`, `u64`, `i64` as named |
| IDs | The 16 bytes of the UUID |
| Byte strings | A `u32` length, then the bytes |
| Text | A `u32` length, then UTF-8 |
| Optional value | `0x00`, or `0x01` then the value |
| Amount | The currency's ISO 4217 code as text, then the minor units as `i64` |

**Tags.** Every message starts with its tag, so no signature over one kind
can pass for another:

| Message | Tag |
|---|---|
| Issuer certificate | `mezgeb.offline.issuer.v1\0` |
| Device certificate | `mezgeb.offline.device.v1\0` |
| Voucher | `mezgeb.offline.voucher.v1\0` |
| Receipt | `mezgeb.offline.receipt.v1\0` |
| Payee acknowledgement | `mezgeb.offline.ack.v1\0` |
| Voucher close | `mezgeb.offline.close.v1\0` |

A message's **body** is its tag and fields in the order listed below. Its
**digest** is the SHA-256 of the body. Its signature is over the body. The
signature is never part of the digest, so two signatures of one body are the
same message.

## 4. Keys

```
root key (P-256, offline, cold storage)
  └─ signs issuer certificates
       issuer keys (P-256, online in the ledger; key service from step 3.12)
         ├─ sign device certificates
         └─ sign vouchers
device keys (P-256, generated in the device's secure hardware, never exported)
  ├─ payer: sign receipts and voucher closes
  └─ payee: sign acknowledgements
```

- **Root key.** It signs a certificate for each issuer key. It never touches
  a network. Apps and terminals ship its public key.
- **Issuer keys.** Rotated every 90 days.
  - A retired issuer key verifies, but signs nothing new, until every
    voucher it signed is past its grace period.
  - Issuer certificate fields: `issuer_key_id` (text), `public_key` (bytes),
    `valid_from` (`i64` µs), `valid_until` (`i64` µs).
- **Device keys.** Generated where they will live:
  - tier A: in the card's secure element;
  - tier B: in StrongBox or the TEE;
  - tier C: in the app's software keystore.
- **Revocation.** The ledger publishes revoked device and issuer key IDs.
  Terminals refresh the list whenever they are online. A terminal refuses
  vouchers from a revoked issuer key, and receipts from a revoked device it
  knows of.

## 5. Device registration and attestation (online)

1. **The device generates a P-256 key pair** in its secure hardware.
2. **The client registers it** with `POST /v1/offline/devices`, sending:
   - the public key;
   - the owner's wallet account;
   - a label;
   - the **attestation**:
     - **Card:** the chip's certificate, chained to the card personaliser's
       CA, which the client configured as trusted.
     - **Android:** the Key Attestation certificate chain, to Google's
       attestation roots. Its security level must be `StrongBox` or
       `TrustedEnvironment`, with verified boot and a locked bootloader.
     - **None:** allowed only if the client enables tier C.
3. **The ledger verifies the attestation and assigns the tier:** A for a
   card, B for an attested phone key, C for no attestation.
4. **The ledger issues a device certificate,** signed by the current issuer
   key. Its fields:
   - `device_id` (UUIDv7), `client_id`, `owner_account_id`;
   - `public_key` (bytes), `key_id` (text), `tier` (`u8`: 1 = A, 2 = B, 3 = C);
   - `issued_at` and `valid_until` (`i64` µs; one year);
   - `issuer_key_id` (text).
5. **The device stores its certificate.** It shows the certificate to a
   payee with each payment, so the payee can verify receipts offline.

**Device states:**

```
REGISTERED → ACTIVE → FROZEN (a conflict; operator decides) → REVOKED
                    → LOST (owner reports) ────────────────→ REVOKED
                    → REVOKED (owner or client retires it)
```

- No state but `ACTIVE` can get a new voucher.
- A `FROZEN` device's receipts are kept but not settled, until the dispute
  ends.

## 6. Trust tiers

| | Tier A | Tier B | Tier C |
|---|---|---|---|
| Device | Smart card, secure element | Phone, attested StrongBox/TEE key | Phone, software key |
| Counter | In the secure element, bound to the key | In the app (see below) | In the app |
| Per voucher (max) | 2,000 ETB | 1,000 ETB | 200 ETB |
| Voucher lifetime | 7 days | 3 days | 1 day |
| Grace after expiry | 7 days | 7 days | 7 days |

- **Per customer:** at most 5,000 ETB of outstanding escrow across all
  their devices.
- **Configurable down only.** A client may set lower limits, or disable a
  tier, but never raise a limit above these maxima.
- **Phones have no hardware counter bound to their key.** Android Keystore
  has none. A phone restored from a backup, or cloned, can therefore repeat
  a counter. The ledger catches it, after the fact, as a conflict (§10).
  Tiers B and C get lower limits for this reason.

## 7. Voucher (issued online)

The client calls `POST /v1/offline/vouchers` for an `ACTIVE` device with no
open voucher. The ledger checks:

- the tier limit and the per-customer cap;
- that the payer has no unpaid shortfall (§12).

Then it posts **E16**, moving the amount from the wallet to the voucher's
escrow sub-account `2050:vch-<id>`, and signs the voucher. In one
transaction.

**Voucher fields, in order:**

| Field | Encoding | Meaning |
|---|---|---|
| `voucher_id` | ID | UUIDv7 |
| `device_id` | ID | The device it is bound to |
| `client_id` | ID | |
| `escrow_account_id` | ID | `2050:vch-…` |
| `amount` | amount | The allowance |
| `counter_start` | `u64` | The first counter it authorises: after the device's last counter (see below) |
| `issued_at` | `i64` µs | Ledger time |
| `expires_at` | `i64` µs | Ledger time; `issued_at` + the tier's lifetime |
| `issuer_key_id` | text | |

`signature`: the issuer key's signature over the body.

**The first counter.** The request carries the device's `last_counter`: 0
if it has never signed a receipt. `counter_start` is the highest of:
- `last_counter` + 1;
- the highest counter the ledger has recorded for the device, + 1;
- the device's `next_counter`, which rises with every receipt recorded and
  with every voucher issued.

So a receipt of an earlier voucher still in transit never shares a counter
with the new voucher: it arrives later and is settled under its own
voucher's rules. A device that reports too little (rolled back, or a clone)
still starts after what the ledger has recorded.

**Loading the voucher.**
- **The device refuses a voucher whose `counter_start` is not above its own
  counter**; that would mean the ledger saw counters the device did not
  sign. It then continues from `counter_start`.
- **On a card:** the chip stores the voucher and sets
  `remaining = amount`.
- **On a phone:** the app stores it in its protected storage.

**Voucher states:**

```
                 (owner closes online, §7.1)
ACTIVE ──────────────────────────────────────────→ CLOSED
  │ expires_at passes (ledger time)                  │ unspent escrow returned (E18)
  ▼                                                  ▼
GRACE ── grace ends ──→ RETURNED (E18) ── 90 days ──→ ENDED (late receipts refused)
  │
  └── device frozen ──→ FROZEN ── dispute resolved ──→ RETURNED
```

- **`ACTIVE`, `GRACE`:** receipts settle from escrow.
- **`RETURNED`:** receipts settle under the loss policy (§12).
- **`ENDED`:** late receipts are refused as `EXPIRED`.
- **`FROZEN`:** nothing settles until the dispute ends.

### 7.1 Closing a voucher early (online)

The device signs a **close** message with these fields:
- `voucher_id`;
- `last_counter` (`u64`; `counter_start - 1` if it spent nothing);
- `last_receipt_digest` (32 bytes; the voucher's digest if none).

The ledger then:

1. accepts the close once every receipt up to `last_counter` has settled
   (the device uploads them with the close);
2. returns the unspent escrow (**E25**);
3. marks the voucher `CLOSED`.

The device's next voucher starts at `last_counter + 1`.

**A close without the device** (it is lost) is not possible, because the
device may have signed receipts nobody has uploaded yet. The owner can only
report it `LOST` (§13).

## 8. Receipt (offline, payer to payee)

**At the moment of payment:**

1. The payee sends the payer:
   - its account and device ID;
   - the amount;
   - a fresh 16-byte random `payee_challenge`.
2. The payer's device checks that `amount ≤ remaining`.
3. In one atomic step, the device:
   - increments its counter;
   - computes the receipt;
   - signs it;
   - stores it as its new `last_receipt_digest`;
   - reduces `remaining`.

**Receipt fields, in order:**

| Field | Encoding | Meaning |
|---|---|---|
| `device_id` | ID | The payer's device |
| `voucher_id` | ID | |
| `counter` | `u64` | Strictly increasing; first is `counter_start` |
| `prev_digest` | 32 bytes | The previous receipt's digest; for the voucher's first receipt, the **voucher's digest** |
| `payee_account_id` | ID | Who is paid |
| `payee_device_id` | optional ID | The payee terminal, if it has one |
| `amount` | amount | In the voucher's currency |
| `event_time` | `i64` µs | The device's clock: recorded, never trusted |
| `payee_challenge` | 16 bytes | From the payee, so the receipt is fresh and for this payment |

`signature`: the payer device key's signature over the body.

**Delivery.** The payer gives the payee:
- the receipt and its signature;
- the voucher and its issuer signature;
- the payer's device certificate.

**What the payee checks offline:**
1. the issuer key against the root;
2. the device certificate, and that the device is not revoked;
3. the voucher's signature, and that the voucher names this device;
4. the receipt's signature, low-S;
5. its `payee_challenge`, account and amount;
6. that `event_time` is before `expires_at` by the payee's own clock (a
   warning only).

**The acknowledgement (optional).** The payee device signs:
- `receipt_digest` (32 bytes);
- `payee_device_id`;
- `received_at` (`i64` µs, its clock).

It gives the ack to the payer. The ack proves the payee received the
receipt. It is evidence in a dispute, never needed for settlement.

## 9. Upload and settlement (online, either party)

`POST /v1/offline/receipts` takes a batch of receipts, with their vouchers
and device certificates. Each receipt is processed on its own, idempotently
by `(device_id, counter)`, and gets one **outcome**.

**The checks, in order:**

| # | Check | Outcome if it fails |
|---|---|---|
| 1 | Version and encoding | `BAD_FORMAT` |
| 2 | The device is known to the ledger | `UNKNOWN_DEVICE` |
| 3 | The signature verifies with the device's key, low-S | `BAD_SIGNATURE` |
| 4 | The voucher is the device's, and `counter ≥ counter_start` | `UNKNOWN_VOUCHER` |
| 5 | `(device_id, counter)` already recorded with the same digest | `DUPLICATE` (nothing posted) |
| 6 | Already recorded with a **different** digest | `CONFLICT`: the device and voucher freeze (§10) |
| 7 | The device or voucher is frozen | `FROZEN`: kept for the dispute |
| 8 | An earlier counter of the voucher is missing | `WAITING_FOR_GAP`: kept (§11) |
| 9 | `prev_digest` equals the previous receipt's digest | `CONFLICT` if not |
| 10 | `ACTIVE`/`GRACE`, amount ≤ escrow left | `SETTLED`: **E17** from escrow |
| 11 | `ACTIVE`/`GRACE`, amount > escrow left | `OVER_LIMIT`: the device freezes (§10) |
| 12 | `RETURNED` | `SETTLED_LATE`: **E28/E29** under the loss policy (§12) |
| 13 | `ENDED` (past the late deadline) | `EXPIRED`: nothing posted, payee bears it |

**Each receipt is stored the first time it arrives,** whatever its outcome,
with its digest, by `(device_id, counter)`. That record is the basis for
`DUPLICATE` and `CONFLICT`.

**The settlement entry** (E17) is a merchant-payment booking from escrow.
- Its idempotency key is `offline:<device_id>:<counter>`, so a retried
  upload posts nothing twice (INV-20).
- Its `event_time` is the device's `event_time`, when that lies between the
  voucher's `issued_at` and the ledger's now. Otherwise it is left empty,
  and the claimed time is kept in metadata.

**The same result whoever uploads** (spec §5.5). The checks use only the
receipt and the ledger's state, never who sent it.

## 10. Conflicts and over-spending

A correct device never signs two receipts with one counter (§8, atomic
step), and never more than its voucher. Either happening means the device
was **cloned, rolled back** (a phone restored from a backup) or its hardware
was broken.

- **`CONFLICT`:** a second receipt for a counter, with a different digest;
  or a receipt whose `prev_digest` does not match.
- **`OVER_LIMIT`:** a receipt larger than the escrow left.

**On either, at once:**
1. the device becomes `FROZEN`, and its voucher `FROZEN`;
2. receipts already settled stay settled;
3. everything else of that voucher is kept unsettled, both waiting receipts
   and new uploads;
4. the client and the operator are alerted.

The **dispute path** (§14) decides what happens next.

## 11. Gaps

**Order.** Receipts of a voucher settle in counter order, because each
commits to the previous digest. A receipt whose predecessor has not arrived
waits as `WAITING_FOR_GAP`.

**The gap timeout.** A gap stops waiting after **72 hours** from the first
receipt that waited on it, or when the voucher's escrow is returned,
whichever comes first. The 72 hours are ledger time, from when the waiting
receipt reached the ledger (`received_at`), like every other deadline
here. The waiting receipts then settle in counter order:
- **skipping the gap**, so the gap's own `prev_digest` link is not checked;
- with every other check still applied (signature, no conflict, escrow);
- each marked `gap_skipped`.

**When a missing receipt arrives after its gap was skipped:**
- **Its digest must equal the `prev_digest` of its successor.** If not, it
  is a `CONFLICT`.
- If it matches, it settles from the escrow left, or under the loss policy
  if the voucher was returned.

## 12. Loss policy (late receipts)

A valid receipt arriving after its voucher was `RETURNED` (and before
`ENDED`) is paid to the payee as follows:

1. **From the payer's wallet**, as far as its balance allows:
   **E28** (Dr payer wallet, Cr payee).
2. **The rest from the platform's offline loss account,** 5030:
   **E29** (Dr 5030, Cr payee).
3. **The shortfall** (step 2) is recorded against the payer: its amount,
   the receipt and the device. Until the payer repays it, **no device of
   theirs gets a new voucher**. Repayment is a transfer from the wallet,
   **E30** (Dr payer wallet, Cr 5030).

The merchant fee applies as in E17. The receipt's outcome is
`SETTLED_LATE`, and it records the parts paid from each source.

**The late deadline.** After 90 days past the return (a client setting, at
most 365 days), the voucher is `ENDED`. Its late receipts are refused as
`EXPIRED`: the payee bears them. That bounds the platform's exposure in
time.

## 13. Lost device

The owner (through the client) reports the device `LOST`. Then:

- **No new voucher** for it, and its certificate is revoked.
- **The current voucher stays as it is.** Its receipts still settle until
  the voucher returns, because payees accepted them in good faith. Whoever
  holds the device can spend at most what is left in escrow, like cash in a
  lost wallet. Cards should require a PIN above a small amount (§15).
- **The voucher is not closed early.** Receipts may still be travelling, so
  it expires and returns normally.

## 14. Dispute path

A frozen device's case goes to the operator, who sees:
- every receipt of the voucher, settled and unsettled;
- the conflicting pairs;
- any payee acknowledgements.

| Finding | Resolution | Entries |
|---|---|---|
| A cloned or rolled-back device: two valid receipts, one counter | The settled one stands. Each other valid receipt is paid to its payee under the loss policy, as a late receipt (payer liable, as for any receipt their device signed). The device is revoked | **E31** (E28/E29 shape) |
| An over-spend | Paid in counter order up to the escrow. The receipt that crosses it is paid partly from escrow and partly under the loss policy | E17, then **E26** |
| A receipt the payer contests ("I never paid") | Valid signatures stand: the device signed it. The client handles the customer's complaint outside the ledger | none |
| A fault in the ledger or the hardware vendor's chip | Operator adjustment, two people (INV-25) | adjustment |

**The late deadline still holds.** A receipt or a conflicting copy that
reached the ledger after the voucher `ENDED` is never paid, however the
dispute ends: a recorded one becomes `EXPIRED`, and its payee bears it, as
for any receipt past the deadline (§12). What arrived before the end is
paid, however long the operator took.

**When the dispute ends:**
1. if the voucher is still open (`ACTIVE`, `GRACE` or `FROZEN`), its
   remaining escrow is returned (**E27**) and it becomes `RETURNED`, then
   `ENDED` normally. A voucher that had already returned or ended keeps its
   state and its return time, so its late deadline does not move;
2. the device is revoked;
3. receipts of the device's other vouchers, kept `FROZEN` only because the
   device was frozen, are decided as if they had just arrived: `EXPIRED` if
   their voucher has ended.

## 15. Card hardware requirements

What a card chip must do for tier A. To confirm with the vendor before
cards are ordered.

| # | Requirement | Why |
|---|---|---|
| H1 | Generate a P-256 key pair on the chip; the private key is never exportable | Device key (§4) |
| H2 | ECDSA P-256 signatures over SHA-256, low-S (or the terminal normalises `s`) | Receipts (§2) |
| H3 | A monotonic counter bound to the key: incrementing it, signing and storing the new state happen as one tear-safe step (power loss cannot roll it back) | No repeated counter (INV-20, INV-21) |
| H4 | Store the voucher, `remaining` and `last_receipt_digest`; refuse to sign above `remaining` | No over-spending (INV-19) |
| H5 | A chip attestation certificate, signed by the personaliser's CA at personalisation | Tier A registration (§5) |
| H6 | Cardholder verification (PIN) above a configurable amount | Lost card (§13) |
| H7 | Contactless ISO/IEC 14443; one payment within 500 ms | Tap to pay |
| H8 | Room for at least the last 50 receipts, readable by the payer's phone | Payer-side upload and disputes |

**Phones (tiers B, C).** H1 and H2 come from Android Keystore. H3 and H4
are done in the app, which is why phone tiers have lower limits (§6).

## 16. What the ledger records (steps 2.11–2.13)

| Record | Fields |
|---|---|
| **Devices** | ID, client, owner account, public key, key ID, tier, state, attestation summary, certificate |
| **Vouchers** | ID, device, escrow account, amount, counter start, issued, expires, state, the signed body |
| **Offline receipts** | `(device_id, counter)` unique; digest, body, signature, outcome, the entry that settled it, and how much came from escrow, the wallet and 5030 |
| **Shortfalls** | Payer, amount, receipt; repaid by which entry |

All are append-only, with state changes as audited events (ADR-0018).
