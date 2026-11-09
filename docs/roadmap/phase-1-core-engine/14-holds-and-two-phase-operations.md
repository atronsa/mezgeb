# 1.14 Holds and Two-Phase Operations

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.13 Balance Verification and Integrity Checking](13-balance-verification.md) · Next: [1.15 Failure Injection and Crash Recovery for the Core](15-failure-injection-and-crash-recovery.md)

---

**1. Overall Objective**
Reserve funds now and capture or release them later. Card pre-auth, pending withdrawals and offline escrow all build on this.

**2. The Problem We Are Solving**
Funds promised but not yet moved must not be spendable twice, and must never get stuck forever.

**3. Concepts and Research**
- Authorisation vs capture.
- Available vs ledger balance.
- Expiry.
- Partial capture.
- Holds as postings to `…:held` sub-accounts (ADR-0004).
- Idempotent background workers.
- `SKIP LOCKED` job claiming.

**4. Implementation Tasks**
- `holds` table (id, account, held account, amount, status `active|captured|released|expired`, expires_at, entry ids).
- `place_hold` (an entry from wallet to held).
- `capture_hold` (full or partial; an entry from held to destination, with the remainder released).
- `release_hold`.
- An expiry worker that claims due holds with `FOR UPDATE SKIP LOCKED` and releases them idempotently.
- Status transitions enforced in the engine and the DB.

**5. Testing Requirements**
- Hold, then spend: available is reduced.
- Capturing twice: the second is rejected.
- Capture racing expiry concurrently: exactly one wins.
- Partial capture plus release equals the original.
- The worker crashes mid-batch and is restarted: no hold is released twice.
- Property test: the held account balance always equals SUM(active holds).

**6. Deliverables**
Holds module, expiry worker and tests.

**7. Acceptance Criteria**
The invariant "held balance = sum of active holds" holds in the oracle and in PG under concurrency.

**8. Dependencies**
Steps [1.10](../phase-1-core-engine/10-concurrency-control.md)–[1.13](../phase-1-core-engine/13-balance-verification.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The race test between capture and expiry passes 50 consecutive runs.
