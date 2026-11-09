# 2.09 Audit Logging and Tamper Evidence

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.08 Authentication, Authorization and Privilege Separation](08-authentication-and-authorization.md) · Next: [2.10 Offline Architecture and Protocol Design (Design Only)](10-offline-protocol-design.md)

---

**1. Overall Objective**
Record who did what and when, and make silent tampering detectable.

**2. The Problem We Are Solving**
A database superuser can rewrite history without leaving a trace. Auditors
need evidence that this didn't happen.

**3. Concepts and Research**
- Append-only audit events.
- Hash chains (each entry hash covers the previous hash).
- Signed checkpoints (Ed25519).
- External anchoring (publishing checkpoint hashes outside the DB).
- What tamper *evidence* can and cannot prove, as opposed to tamper *proof*.

**4. Implementation Tasks**
- `audit_events` table: actor, client, action, target, request ID, outcome, before/after for non-ledger config. Append-only.
- A hash chain on `journal_entries` (`prev_hash`, `hash`) computed in the posting transaction (the chain head row is locked; measure the contention cost).
- Daily signed checkpoints.
- `mezgeb-cli verify --chain`.

**5. Testing Requirements**
- Tampering with any entry, line or audit row (as superuser) is detected by the chain verify.
- Deleting the last entry is detected via the checkpoint.
- Benchmark posting throughput with and without the chain. If the chain head becomes a bottleneck, record it for step 3.03.

**6. Deliverables**
Audit module, hash chain and checkpoint signer.

**7. Acceptance Criteria**
All planted tampering is detected, and every API mutation produces an audit event.

**8. Dependencies**
Steps [1.13](../phase-1-core-engine/13-balance-verification.md), [2.08](../phase-2-operations-and-security/08-authentication-and-authorization.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
Where the checkpoint is published is decided (ADR).
