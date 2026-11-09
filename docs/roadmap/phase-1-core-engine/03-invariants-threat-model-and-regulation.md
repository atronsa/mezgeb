# 1.03 Invariant Catalogue, Threat Model v0, Regulatory Applicability

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.02 Architecture Decision Records and Repository Restructure](02-architecture-decisions.md) · Next: [1.04 Money, Currency and Identifier Types](04-money-and-identifier-types.md)

---

**1. Overall Objective**
Define what "correct" means precisely, who might attack the ledger, and what law actually applies.

**2. The Problem We Are Solving**
Tests without stated invariants only test the cases you thought of. Compliance assumptions made without research produce the wrong design or false claims.

**3. Concepts and Research**
- Invariants vs validations.
- STRIDE threat modelling.
- Research National Bank of Ethiopia directives for payment instrument issuers: stored value, offline limits, record retention, reporting. **Confirm applicability with a legal advisor; do not assume.**
- ISO 4217 for ETB.

**4. Implementation Tasks**
- Rewrite spec §2 as `docs/invariants.md`. For each invariant (INV-n): statement, where it is enforced (type / engine / DB / job), and how it is tested. Cover balance, immutability, idempotency, non-negative, period closure, holds, reversal uniqueness, refund cap and offline.
- Write `docs/threat-model.md` with actors (external attacker, compromised client server, malicious insider admin, malicious device owner, colluding merchant) and assets (balances, keys, audit log).
- Write `docs/regulatory.md` listing questions, sources, answers and status.

**5. Testing Requirements**
- Traceability table: every invariant has at least one planned test ID.
- Every threat has at least one planned control.

**6. Deliverables**
Three documents and the traceability matrix.

**7. Acceptance Criteria**
At least 15 invariants, at least 20 threats each with an owner and control, and the regulatory questions logged with a status.

**8. Dependencies**
Steps [1.01](../phase-1-core-engine/01-accounting-foundations.md)–[1.02](../phase-1-core-engine/02-architecture-decisions.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The invariant list is reviewed and frozen at v1 (changes go through ADRs from now on).
