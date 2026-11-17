# Runbook: Offline Payments

[Runbooks](README.md) · Code: [`crates/cli/src/offline.rs`](../../crates/cli/src/offline.rs), [`crates/engine/src/offline.rs`](../../crates/engine/src/offline.rs) · Step: [2.11](../roadmap/phase-2-operations-and-security/11-device-registry-and-vouchers.md) · Decision: [ADR-0019](../adr/0019-offline-protocol-v1.md) · Protocol: [offline protocol v1](../offline/protocol.md)

Setting up offline payments and looking after them: the issuer key, which
tiers each client may use, freezing and revoking devices, and checking that
every voucher's escrow holds what it should.

| | |
|---|---|
| Who | The operator's administrator; the verifier for `check` |
| Command | `mezgeb-cli offline ...` |
| Connects as | `mezgeb_admin` for `tier` and `device`; `mezgeb_verifier` for `check` |
| Takes | Seconds |

## 1. The issuer key, once

The issuer key is a P-256 key that signs every device certificate and
voucher. It is not the Ed25519 signing key of receipts and checkpoints
(ADR-0018): each has its own file.

```sh
mezgeb-cli offline key generate --out /secrets/offline-issuer.hex
```

- The command never overwrites a file.
- Give the service the path in `MEZGEB_OFFLINE_ISSUER_KEY_FILE`. On start it
  publishes the public half in `offline_issuer_keys`, and logs
  `offline issuer key loaded` with the key ID.
- Keep the file as secret as the signing key: anyone with it can sign
  vouchers that devices and payees will accept.
- Without the variable, the service runs with offline payments off: the
  offline endpoints answer `503 offline_unavailable`.

**Rotation (every 90 days, protocol §4).** Generate a new key, point
`MEZGEB_OFFLINE_ISSUER_KEY_FILE` at it and restart. The old public key stays
listed by `GET /v1/offline/issuer-keys`, so the vouchers it signed still
verify. Keep the old file until every voucher it signed is past its grace
period, then destroy it.

**The accounts.** Before turning offline payments on, open the platform's
fee revenue (4010) and offline loss (5030) accounts, as for any system
account:

```sh
curl -X POST .../v1/accounts -d '{"reference": "4010"}'    # with an operator token
curl -X POST .../v1/accounts -d '{"reference": "5030"}'
```

Without 5030, a late receipt whose payer cannot pay it all fails with a
500, and stays uploadable.

**The tick.** The service moves vouchers through time by itself, every
`MEZGEB_OFFLINE_TICK_SECONDS` (60 by default; `0` turns it off):

- receipts waiting 72 hours for a missing one settle past it;
- vouchers expire into grace, and 7 days later return their escrow (E18);
- 90 days after the return (the client's late deadline), late receipts
  stop being paid.

It logs `offline tick` with counts whenever it did something. If several
service instances run, each ticks; that is safe, because each voucher is
handled under its locks and nothing is done twice.

## 2. A client's tiers

| Tier | Device | Default |
|---|---|---|
| A | Smart card | On: 2,000 ETB, 7 days |
| B | Attested phone | On: 1,000 ETB, 3 days |
| C | Phone, software key | **Off** |

Until a card chip is chosen and Android Key Attestation is verified, the API
registers devices with no attestation only, as tier C. So a client that
wants offline payments now needs tier C enabled:

```sh
mezgeb-cli offline tier wallet-app C --enable                 # 200 ETB, 1 day
mezgeb-cli offline tier wallet-app C --enable --max 100 --lifetime-hours 12
mezgeb-cli offline tier wallet-app C --disable
```

- Limits can be lowered, never raised above the protocol's maxima; the
  command refuses before it connects.
- A new limit applies to the next voucher. Vouchers already issued keep
  theirs.
- Disabling a tier stops new vouchers for its devices; it does not touch
  open vouchers.
- Every change is an audit event (`offline.tier`).

## 3. The merchant fee

A client may take a fee from offline payments, as from online ones (E07):
the merchant is paid the amount less the fee, and the fee is credited to
4010.

```sh
mezgeb-cli offline fee wallet-app --bps 200    # 2%
mezgeb-cli offline fee wallet-app --bps 0      # none
```

- In basis points, from 0 to 1,000 (10%). Without a setting, no fee.
- The fee is rounded down to the santim: a fraction stays with the payee.
- A new fee applies to receipts settled from then on.
- Every change is an audit event (`offline.fee`).

## 4. The late deadline

How long after a voucher's return a late receipt is still paid, from the
payer's wallet and then 5030 (protocol §12):

```sh
mezgeb-cli offline deadline wallet-app --days 30    # 1 to 365; 90 without it
```

A shorter deadline bounds the platform's exposure sooner; a longer one is
kinder to merchants whose terminals sync rarely.

## 5. Devices

A client registers and revokes its own devices through the API. The
operator acts on any client's device:

```sh
mezgeb-cli offline device freeze    <device-id> --reason "conflict under review"
mezgeb-cli offline device unfreeze  <device-id>
mezgeb-cli offline device lost      <device-id> --reason "owner report 2026-11-16"
mezgeb-cli offline device revoke    <device-id> --reason "replaced"
```

| From | May become |
|---|---|
| `active` | `frozen`, `lost`, `revoked` |
| `frozen` | `active`, `revoked` |
| `lost` | `revoked` |
| `revoked` | nothing: final |

- Only an `active` device gets a voucher.
- None of these closes an open voucher early: receipts may still be on
  their way (protocol §13). The voucher expires and returns its escrow as
  usual (steps 2.12–2.13).
- Every change is an audit event (`offline.device`).

## 6. Checking escrow

```sh
mezgeb-cli offline check --database-url "$VERIFIER_URL"
```

It checks that **each voucher's escrow holds exactly its unspent value**
(its amount less what its settled receipts took), and that no `2050` account
holds money without a voucher.

- **Exit 0:** `every escrow holds its unspent voucher value`.
- **Exit 1:** one line per escrow that disagrees, with what it holds and
  what it should hold.

**On a finding.** Do not post anything to fix it. Money in escrow belongs
to the customer, and a voucher's escrow moves only by its own entries.
1. Run `mezgeb-cli verify --chain` to rule out tampering.
2. Find the entries that touched the escrow, from `GET /v1/accounts/{id}/ledger`.
3. Treat it as an incident: an escrow that disagrees with its voucher means
   a posting went around the voucher rules.

## 7. A frozen device

The ledger freezes a device and its voucher by itself when an upload shows:

- **`CONFLICT`:** two different receipts with one counter, or a receipt
  whose link does not match its predecessor. The device was cloned or
  rolled back (a phone restored from a backup).
- **`OVER_LIMIT`:** a receipt larger than the escrow left.

Each freeze writes an `offline.freeze` audit event naming the device, the
voucher and the counter: that is the alert. Read them with
`GET /v1/audit-events` (auditor).

**What is kept.** Receipts already settled stay settled. The conflicting
receipt is kept in `offline_receipt_conflicts`, and every later receipt of
the voucher is kept as `FROZEN`. `GET /v1/offline/vouchers/{id}/receipts`
lists them all.

**The case.** Each freeze opens a dispute. An operator (`offline:resolve`)
lists them with `GET /v1/offline/disputes?status=open`, and reads the
voucher's receipts with `GET /v1/offline/vouchers/{id}/receipts`.

**Deciding it.** Do not unfreeze the device to let payments through.
Resolve the case:

```sh
curl -X POST .../v1/offline/disputes/<id>/resolve -d '{"resolution": "paid"}'
```

| Resolution | When | What happens |
|---|---|---|
| `paid` | The receipts are validly signed: a cloned or rolled-back device, an over-spend. The payer is liable for what their device signed | Every receipt of the voucher is paid in counter order: from the escrow left, then the payer's wallet, then 5030 (recorded as a shortfall). A conflicting copy is paid from the wallet and 5030 only |
| `refused` | A fault in the ledger or the chip, established by the operator | No receipt is paid; they become `REFUSED`, and their payees bear them |

Either way:
- **The escrow left of an open voucher goes back** to the wallet (E27). A
  voucher that had already returned or ended keeps its state and its
  return time: resolving late never extends its late deadline.
- **Nothing that reached the ledger after the voucher ended is paid**, even
  under `paid`: it becomes `EXPIRED`, and its payee bears it (protocol
  §12). What arrived before the end is paid, however long the case took.
- **The device is revoked**, and an `offline.dispute` audit event names who
  decided.
- **The device's receipts on its other vouchers**, kept `FROZEN` while it
  was frozen, are decided at once, as if they had just arrived.

Resolving it again the same way changes nothing; the other way is refused.

## 8. Shortfalls

When a late receipt's payer cannot pay it all, 5030 pays the rest and a
shortfall is recorded against the payer. **Until it is repaid, none of the
payer's devices gets a voucher** (`409 shortfall_unpaid`).

- The client lists them: `GET /v1/offline/shortfalls?owner_account=...`.
- The payer repays from their wallet:
  `POST /v1/offline/shortfalls/<id>/repay` (E30), once the wallet holds it.

5030's balance is the platform's open offline loss: watch it, and the
merchants whose late uploads cause it (step 2.15).
