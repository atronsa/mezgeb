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

## 3. Devices

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

## 4. Checking escrow

```sh
mezgeb-cli offline check --database-url "$VERIFIER_URL"
```

It checks the acceptance criterion of step 2.11: **each voucher's escrow
holds exactly its unspent value**, and no `2050` account holds money without
a voucher.

- **Exit 0:** `every escrow holds its unspent voucher value`.
- **Exit 1:** one line per escrow that disagrees, with what it holds and
  what it should hold.

**On a finding.** Do not post anything to fix it. Money in escrow belongs
to the customer, and a voucher's escrow moves only by its own entries.
1. Run `mezgeb-cli verify --chain` to rule out tampering.
2. Find the entries that touched the escrow, from `GET /v1/accounts/{id}/ledger`.
3. Treat it as an incident: an escrow that disagrees with its voucher means
   a posting went around the voucher rules.
