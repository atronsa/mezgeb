# Finalization (Weeks 49–52)

**Goal:** evidence-based production readiness and a go/no-go decision.
Shadow mode keeps running throughout.

---

## Week 49: Full-System Fault Injection and Recovery Game Day

**1. Overall Objective**
Break the whole production-like system on purpose and prove it recovers
correctly.

**2. The Problem We Are Solving**
Component tests miss interactions between components: client retries racing
ledger restarts, workers resuming after failover, and backlogs after outages.

**3. Concepts and Research**
- Game day planning.
- Blast radius.
- Steady-state hypothesis.
- Failure catalogue: DB failover, disk full, network partition between the client and the ledger, clock skew, worker crash, a mass offline sync burst (e.g. after a regional network outage).

**4. Implementation Tasks**
- Write the game-day plan.
- Run each failure in staging under production-shaped load.
- Record detection time, recovery time and the result of the integrity checks.
- Write follow-up issues.

**5. Testing Requirements**
For every failure:
- verify is clean
- no acknowledged request is lost
- no duplicates
- the alert fired
- the runbook worked

Mass sync burst: 100k receipts in 10 minutes ingest correctly within the target.

**6. Deliverables**
Game-day report.

**7. Acceptance Criteria**
All failures recovered with clean verification, and all follow-ups fixed or explicitly accepted.

**8. Dependencies**
Weeks 41–48.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
No open critical follow-ups.

---

## Week 50: Security Assessment

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
Weeks 32, 44.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
If no external review happened, the readiness report must say so plainly.

---

## Week 51: Runbooks, Operational Documentation and Readiness Review

**1. Overall Objective**
Make sure the system can be operated at 3 a.m. by following documents
alone.

**2. The Problem We Are Solving**
The knowledge lives in one person's head, which is a single point of failure.

**3. Concepts and Research**
- Runbook structure (symptom → diagnosis → action → verification).
- Incident severity levels.
- Post-incident review practice.

**4. Implementation Tasks**
Complete runbooks for:
- every alert
- period close
- reconciliation breaks
- offline conflicts and disputes
- key rotation
- restore
- deploy and rollback
- partition maintenance
- emergency freeze (stop all postings safely)

Also:
- Write the incident response plan.
- Go through the production readiness checklist (README) item by item, attaching evidence.

**5. Testing Requirements**
A tabletop exercise: walk through 3 incidents using only the runbooks, and fix the gaps found.

**6. Deliverables**
Runbooks, incident plan and the completed checklist with evidence links.

**7. Acceptance Criteria**
Every checklist item is either evidenced or explicitly marked "not met" with a plan.

**8. Dependencies**
Weeks 41–50.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
Readiness review held and recorded.

---

## Week 52: Release Candidate, Cutover Plan and Open-Source Release

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
**The cutover happens only when the go/no-go criteria are met**, which may be after Week 52. Shadow mode should have run at least 30 clean days before the flip.
