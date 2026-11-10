# 0010. Offline payments settle from escrow by signed receipts

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-10 |
| Answers | [Architecture §5.8](../architecture.md#58-offline-transaction-synchronisation) |
| Implemented in | 2.10–2.14 (offline vouchers, receipts, sync, conflicts, simulation) |

## Context

Mezgeb must support payments when the payer's device has no connection. In
that case:

- There is no central lock, so the ledger cannot stop a double spend at the
  moment of payment.
- The device's clock, its storage and even its owner may be hostile.
- What arrives later is a **claim** that a payment happened, not a payment.

An offline payment therefore cannot be treated as a delayed online
transaction.

## Decision

**Escrow, signed claims and deterministic settlement:**

1. **Reserve.** While online, the ledger moves an offline allowance from the
   wallet into a per-voucher escrow account and issues a **signed voucher**
   bound to one device, with an amount and an expiry (E16).
2. **Pay offline.** The device's secure hardware signs **receipts** with a
   monotonic counter, chained by hash to the previous receipt.
3. **Settle.** When a receipt arrives, from the payer or the payee, in any
   order and any number of times, the ledger verifies the signature,
   voucher, counter and chain, then settles from escrow to the payee
   **exactly once** (E17).
4. **Handle anomalies.** Duplicates are ignored. Gaps in the counter wait.
   A conflict (same counter, different content) freezes the device and goes
   to dispute.
5. **Bound the risk.** Loss is limited by the escrow amount and by limits per
   device trust tier. Unspent escrow returns to the wallet on expiry (E18).
   Receipts arriving after the return go to suspense and are handled under
   a written loss policy.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Escrow + signed receipts (chosen)** | The ledger never pays more than it reserved. Settlement is deterministic and repeatable. Works whichever side syncs first. | Money is locked in escrow until spent or expired. Needs device keys and secure hardware. |
| Trust the device's balance (offline debit, reconcile later) | Simple. No money locked. | A tampered or cloned device can spend without limit. Losses are unbounded. |
| Credit limit (allow the wallet to go negative offline) | No reservation needed. | Turns every offline user into a borrower. Needs credit policy and collections. |
| No offline payments | Nothing to build. | Fails a core requirement for areas with poor coverage. |

## Consequences

**Positive:**

- The worst case is known in advance: the total escrow outstanding.
- Settlement is idempotent by `(device_id, counter)` (ADR-0008).

**Negative:**

- Customers see part of their balance locked while offline allowances are
  active.
- Key management, device registration and the loss policy must be designed
  (steps 2.10–2.13).
- Late receipts after expiry can create small losses (account 5030, reserved
  in the chart of accounts).
