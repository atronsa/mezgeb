# 4.02 Security Assessment

[Phase 4: Finalization](README.md) · Previous: [4.01 Full-System Fault Injection and Recovery Game Day](01-game-day.md) · Next: [4.03 Runbooks, Operational Documentation and Readiness Review](03-runbooks-and-readiness-review.md)

---

**1. Overall Objective**
Get the strongest independent security evidence affordable before going
live.

**2. The Problem We Are Solving**
A solo developer cannot effectively audit their own security assumptions.

**3. Concepts and Research**
- Scope of a penetration test.
- Cryptographic protocol review (offline).
- Secure code review.
- Supply chain (`cargo vet`/`deny`).
- Responsible disclosure policy for an open-source project.

**4. Implementation Tasks**
- Commission (or at minimum request) an external review of the offline protocol and the API, booked weeks in advance.
- Run an internal pentest from the ASVS checklist.
- Fix findings.
- Write `SECURITY.md` with the disclosure process.

**5. Testing Requirements**
- Retest every finding after its fix.
- Re-run the abuse suite and API fuzzing.

**6. Deliverables**
Assessment report, fixes and `SECURITY.md`.

**7. Acceptance Criteria**
- Zero open critical or high findings.
- Medium findings fixed or risk-accepted in writing.

**8. Dependencies**
Steps [2.16](../phase-2-operations-and-security/16-phase-2-security-review-and-exit.md), [3.12](../phase-3-scale-and-integration/12-secrets-and-key-management.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
If no external review happened, the readiness report must say so plainly.
