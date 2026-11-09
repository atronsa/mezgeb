# 3.13 Client Integration Design and Account Mapping

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.12 Secrets, Key Management and Operational Security](12-secrets-and-key-management.md) · Next: [3.14 Historical Replay and Migration Verification](14-historical-replay.md)

---

**1. Overall Objective**
Plan exactly how the first client application moves from its existing ledger
to Mezgeb.

**2. The Problem We Are Solving**
Migrating live money is the riskiest step of the whole project.

**3. Concepts and Research**
- The client's current ledger schema:
  - accounts
  - transactions and entries
  - cached wallet balances
  - fee and clearing accounts
  - daily close
- Strangler pattern.
- Dual-write vs replay.
- The client-side intent table and retry worker.

**4. Implementation Tasks**
- A mapping document: each legacy account → new account code; each legacy transaction type → template.
- Gaps and mismatches listed and resolved.
- Client-side changes specified: intent table, ledger client, idempotency keys from transaction IDs, the event consumer.
- The cutover plan outline, with rollback.

**5. Testing Requirements**
- Every legacy transaction type in production data has a mapping (an automated scan of distinct types).

**6. Deliverables**
Mapping document and client integration spec.

**7. Acceptance Criteria**
Zero unmapped types or accounts.

**8. Dependencies**
Steps [2.01](../phase-2-operations-and-security/01-posting-templates.md)–[2.05](../phase-2-operations-and-security/05-accounting-periods-and-closing.md), [3.06](../phase-3-scale-and-integration/06-transactional-outbox.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The mapping is reviewed against real (anonymised) production data.
