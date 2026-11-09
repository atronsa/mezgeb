# 3.10 Backups, Point-in-Time Recovery and Disaster Recovery

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.09 Observability: Logs, Metrics, Dashboards, Alerts](09-observability.md) · Next: [3.11 Deployment and CI/CD](11-deployment-and-ci-cd.md)

---

**1. Overall Objective**
Be able to restore the ledger to any point in time and prove the restore is
correct.

**2. The Problem We Are Solving**
An untested backup is a hope, not a backup. Money data cannot be lost.

**3. Concepts and Research**
- WAL archiving.
- Base backups.
- pgBackRest vs WAL-G (choose one).
- RPO and RTO definitions.
- Streaming replication for HA vs DR.
- Encryption of backups.
- Off-site copies.

**4. Implementation Tasks**
- Configure pgBackRest with encrypted, off-site storage.
- Define RPO/RTO in an ADR.
- A restore script that restores to a timestamp, runs `verify` and the chain verify, then compares with the checkpoint.
- A monthly restore drill job.

**5. Testing Requirements**
- Recovery tests: restore to a timestamp mid-load; all entries acknowledged before that time are present and verify is clean.
- Total loss of the primary → restore within the RTO (timed).

**6. Deliverables**
Backup configuration, restore tooling and the DR runbook.

**7. Acceptance Criteria**
Two successful timed restore drills recorded.

**8. Dependencies**
Steps [1.13](../phase-1-core-engine/13-balance-verification.md), [2.09](../phase-2-operations-and-security/09-audit-logging.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
A restore drill is performed by following only the runbook.
