# 4.04 Release Candidate, Cutover Plan and Open-Source Release

[Phase 4: Finalization](README.md) · Previous: [4.03 Runbooks, Operational Documentation and Readiness Review](03-runbooks-and-readiness-review.md)

---

**1. Overall Objective**
Produce v1.0.0-rc, the first client's cutover plan, and the public open-source
release.

**2. The Problem We Are Solving**
Going live must be a reversible, criteria-driven decision, not a deadline.

**3. Concepts and Research**
- Go/no-go criteria.
- Cutover patterns: flip the authoritative ledger behind a flag, with the old ledger becoming the shadow for a period so rollback is possible.
- Semantic versioning.
- Open-source governance (contributing guide, code of conduct, licence).

**4. Implementation Tasks**
- Cutover plan:
  1. Freeze window.
  2. Final replay check.
  3. Flip the flag.
  4. Old ledger becomes the shadow.
  5. Rollback trigger conditions and procedure.
- Write the go/no-go criteria.
- Tag v1.0.0-rc.1.
- Public repo hygiene: README, LICENSE, CONTRIBUTING, SECURITY, architecture docs, examples.

**5. Testing Requirements**
- Rehearse the cutover and the rollback twice in staging.

**6. Deliverables**
Release candidate, cutover plan and the public release.

**7. Acceptance Criteria**
Cutover and rollback rehearsed successfully, and the go/no-go criteria agreed.

**8. Dependencies**
All weeks.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
**The cutover happens only when the go/no-go criteria are met**, which may be after step 4.04. Shadow mode should have run at least 30 clean days before the flip.
