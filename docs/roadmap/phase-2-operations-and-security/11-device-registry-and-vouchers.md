# 2.11 Device Registry and Voucher Issuance

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.10 Offline Architecture and Protocol Design (Design Only)](10-offline-protocol-design.md) · Next: [2.12 Receipt Ingestion and Settlement](12-receipt-ingestion-and-settlement.md)

---

**1. Overall Objective**
Register devices and issue signed offline allowances backed by real escrow.

**2. The Problem We Are Solving**
Offline spending must be backed by money that is already reserved, or the
ledger takes on unbounded risk.

**3. Concepts and Research**
- Ed25519.
- Canonical encoding (fixed binary layout, no ambiguity).
- Key storage for the issuer key (start with an encrypted file / KMS; evaluate an HSM in step 3.12).
- Per-device escrow accounts.

**4. Implementation Tasks**
- `devices` table (id, owner account, tier, public key, attestation result, status `active|frozen|revoked`).
- `vouchers` table.
- `issue_voucher.v1` template: a hold from the wallet into the device escrow, then a signed voucher.
- Tier limits enforced (configured per tier).
- `revoke_device`.
- `mezgeb-offline` canonical encoders.

**5. Testing Requirements**
- Issuing beyond the tier limit is rejected.
- Issuing for a frozen device is rejected.
- Voucher signatures verify, and tampering with a single bit fails.
- Concurrent issuance can't exceed the wallet balance.
- Known-answer test vectors for the encoding, committed so device firmware can test against them.

**6. Deliverables**
Device registry, voucher issuance and test vectors.

**7. Acceptance Criteria**
Escrow balance = SUM(unspent voucher value) at all times (an invariant).

**8. Dependencies**
Step [2.10](../phase-2-operations-and-security/10-offline-protocol-design.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
Test vectors are shared with the card/mobile firmware work.
