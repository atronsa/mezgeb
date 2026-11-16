# 0019. Offline protocol v1: P-256 device signatures, one voucher per device, ordered settlement with a gap timeout, and late receipts paid by the payer first

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-16 |
| Answers | Spec §5; ADR-0010's open questions; E16–E18 open questions; TM-17 to TM-21, TM-23 |
| Implemented in | 2.10 (design), 2.11–2.14 (build) |

## Context

ADR-0010 chose escrow, signed vouchers and signed receipts. Before any code,
the protocol itself must be fixed, because once cards are personalised and
apps shipped it cannot be patched. The open choices: which signatures the
hardware can make, how many vouchers a device holds, in what order receipts
settle when they arrive out of order, how long the ledger waits, and who
pays for a receipt that arrives after its escrow has been returned.

## Decision

1. **ECDSA P-256 with SHA-256 for every device and issuer signature.**
   - Smart-card secure elements and Android StrongBox/TEE support P-256;
     Ed25519 is rare on card chips.
   - Signatures are 64 bytes (`r ‖ s`) with low `s` only, so a signature
     cannot be altered into another valid one.
   - The ledger's own receipts and checkpoints stay Ed25519 (ADR-0018):
     they are signed and checked only by servers.
2. **Three levels of keys.**
   - An offline *root* key certifies the online *issuer* keys.
   - Issuer keys sign device certificates and vouchers, and are rotated.
   - *Device* keys are generated inside the device's secure hardware and
     never leave it.
3. **One active voucher per device.** A device's receipts form one counter
   sequence and one hash chain, anchored to the voucher. A new voucher
   starts where the last one's counter ended, after that voucher is closed
   or returned.
4. **Settlement in counter order, with a gap timeout.**
   - A receipt settles only when every earlier counter of its voucher has
     settled; otherwise it waits.
   - After 72 hours, or when the voucher's escrow is returned, waiting
     receipts settle in order, skipping the gap.
   - A missing receipt that arrives later must match the hash its successor
     committed to.
5. **Ledger time decides, the device clock never does.**
   - A voucher is active until its expiry, then in grace for 7 days, during
     which its receipts still settle from escrow.
   - Then its unspent escrow is returned.
6. **A receipt after the return, the loss policy** (decided with the
   maintainer):
   - The payee is paid if the receipt is valid.
   - The money comes from the payer's wallet as far as it has funds, and
     the rest from the platform's offline loss account (5030).
   - The shortfall is recorded against the payer, who gets no new voucher
     until it is repaid.
   - After 90 days past the return (configurable), a late receipt is
     refused, so losses are bounded in time.
7. **Trust tiers with limits.** The defaults are below; a client may lower
   them, never raise them.

   | Tier | Device | Per voucher | Lifetime |
   |---|---|---:|---|
   | A | Smart card with a secure element | 2,000 ETB | 7 days |
   | B | Phone, attested StrongBox or TEE key | 1,000 ETB | 3 days |
   | C | Phone, software key | 200 ETB | 1 day |

   Every tier has a 7-day grace period. Outstanding escrow per customer,
   across all devices, is capped at 5,000 ETB.
8. **Conflicts freeze the device** (INV-21):
   - two receipts with one counter but different contents;
   - a receipt that overspends its voucher.

   The first receipt to settle stays settled, and the rest go to dispute.
   The payer's device signed both, so the payer is liable under the loss
   policy.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **P-256 (chosen)** | Supported by card secure elements, StrongBox and TEE | Signatures need low-`s` normalisation; slower than Ed25519 |
| Ed25519 for devices | Same scheme as the ledger's receipts | Missing on most card chips and on StrongBox |
| Several vouchers per device at once | Top-ups without waiting | Interleaved counters make gaps and conflicts ambiguous |
| Settle each receipt as it arrives, whatever its counter | Faster payment to merchants | A gap could hide a conflict until after money moved; the chain could not be checked |
| Wait forever for a gap | Every chain fully checked | One lost payee device blocks every later payment |
| Merchant bears late receipts | No platform loss | Merchants learn to refuse offline payments |
| Platform always pays late receipts | Simplest for users | Invites payer–merchant collusion with no recourse |

## Consequences

**Positive:**

- Every outcome is defined, and its accounting is a worked entry
  (E25–E31), or no entry at all.
- The worst case is bounded by the tier limits, the per-customer cap, and
  the late-receipt deadline.

**Negative:**

- Phones have no hardware counter bound to their key. A cloned or
  restored phone can reuse counters. The conflict rule catches it, after
  the fact, and tier limits bound it.
- A merchant may wait up to 72 hours for a payment behind a gap.
- The platform carries some loss (5030) for late receipts when the payer's
  wallet is empty.

**Makes out of date:** spec §5's open points; the open questions under
E16–E18; the reserved status of account 5030, now active for E29 and E30
(it is first posted in step 2.13).
