# 2.16 Phase 2 Security Review and Exit

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.15 Reconciliation (Internal, External, Offline)](15-reconciliation.md) · Next: [3.01 Performance Methodology and Realistic Workload](../phase-3-scale-and-integration/01-performance-methodology.md)

---

**1. Overall Objective**
Review the complete functional system against security standards before
optimising anything.

**2. The Problem We Are Solving**
Security flaws found after scaling and integration are much more expensive to
fix.

**3. Concepts and Research**
- OWASP ASVS Level 2 checklist.
- OWASP API Top 10.
- Business-logic abuse (negative amounts, rounding, races, replay).
- Dependency risk.

**4. Implementation Tasks**
- An ASVS L2 self-assessment written to `docs/security/asvs.md`.
- Abuse-case test suite.
- `cargo audit` and `cargo deny` in CI.
- Fix all high-severity findings.
- Update the threat model.
- Tag `v0.2.0-functional`.

**5. Testing Requirements**
- API fuzzing (property-based random requests against the running service): no 5xx caused by input, and no invariant violation.
- Authorization matrix re-run.
- Race abuse tests (refund, hold, voucher).

**6. Deliverables**
ASVS report, abuse suite and release tag.

**7. Acceptance Criteria**
No open high or critical findings, and every abuse case passes.

**8. Dependencies**
Steps [2.01](../phase-2-operations-and-security/01-posting-templates.md)–[2.15](../phase-2-operations-and-security/15-reconciliation.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate (Phase 2 Gate)**
- Every financial operation is booked through a template.
- The API is secured with default deny.
- Audit chain verified.
- Offline simulator clean for 14 nights.
- Reconciliation working.
