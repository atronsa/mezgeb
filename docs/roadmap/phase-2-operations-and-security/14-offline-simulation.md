# 2.14 Offline Deterministic Simulation

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.13 Gaps, Conflicts, Expiry, Rejections and Disputes](13-offline-conflicts-and-expiry.md) · Next: [2.15 Reconciliation (Internal, External, Offline)](15-reconciliation.md)

---

**1. Overall Objective**
Prove the offline protocol at scale under adversarial conditions.

**2. The Problem We Are Solving**
Hand-written tests cover tens of cases. Real fleets produce millions of
interleavings.

**3. Concepts and Research**
- Deterministic simulation testing (FoundationDB, TigerBeetle VOPR).
- Seeded randomness.
- Simulated clocks.
- Fault schedules.
- Invariant checking every step.
- Reproducing a failure from its seed.

**4. Implementation Tasks**
- `mezgeb-sim` v2:
  - thousands of simulated devices with tiers
  - random payments
  - random sync timing and order
  - duplicate and lost receipts
  - cloned devices
  - clock skew
  - voucher expiry
  - ledger crashes
- Runs against the in-memory model and against PG (slower; nightly).
- Prints the seed and step on failure.
- Nightly CI job.

**5. Testing Requirements**
- 10M simulated operations per nightly run.
- Invariants INV-1 to INV-12 after every step.
- Planted bugs (skip the counter check, skip the chain check) are caught within one nightly run.

**6. Deliverables**
Simulator v2 and the nightly job.

**7. Acceptance Criteria**
14 consecutive clean nightly runs, and every planted bug caught.

**8. Dependencies**
Step [2.13](../phase-2-operations-and-security/13-offline-conflicts-and-expiry.md).

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
A failing seed reproduces identically on another machine.
