# 2.10 Offline Architecture and Protocol Design (Design Only)

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.09 Audit Logging and Tamper Evidence](09-audit-logging.md) · Next: [2.11 Device Registry and Voucher Issuance](11-device-registry-and-vouchers.md)

---

**1. Overall Objective**
Finalise the offline payment protocol and its accounting before writing code.

**2. The Problem We Are Solving**
Offline payments break the online assumptions: no central lock, untrusted
clocks, devices that may be cloned, and data that arrives late or never.
Getting the protocol wrong cannot be patched after cards ship.

**3. Concepts and Research**
- Escrow ("bounded counter") techniques.
- Secure elements and monotonic counters.
- Android Keystore/StrongBox key attestation.
- Offline data authentication in card schemes (as background, not to implement).
- Hash chains.
- Replay attacks.
- Risk limits per trust tier.
- Loss policy for late receipts.

**4. Implementation Tasks**
- Complete spec §5 with:
  - message formats (voucher, receipt) with canonical encoding
  - signature scheme
  - key hierarchy (issuer key, device keys)
  - device registration and attestation
  - trust tiers with limits and expiry
  - full state machines for voucher and receipt
  - the gap timeout
  - conflict handling
  - the dispute path
  - the loss policy
- An offline threat model addendum.
- Worked journal entries for every offline state.
- Align with the card hardware work: confirm the chip supports keys, a counter and signing.

**5. Testing Requirements**
- Desk-check every threat against the protocol.
- Walk 10 adversarial scenarios on paper: clone, replay, forged amount, clock rollback, payee double-upload, payer never syncs, lost device, partial sync, voucher expiry race, colluding merchant.

**6. Deliverables**
Offline spec v1, threat addendum and the scenario walkthroughs.

**7. Acceptance Criteria**
Every scenario has a defined outcome and a defined accounting result.

**8. Dependencies**
Steps [1.14](../phase-1-core-engine/14-holds-and-two-phase-operations.md), [2.09](../phase-2-operations-and-security/09-audit-logging.md); card hardware decisions.

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
External security review of the protocol requested (see README, "Specialist review").
