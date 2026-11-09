# 3.11 Deployment and CI/CD

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.10 Backups, Point-in-Time Recovery and Disaster Recovery](10-backups-and-disaster-recovery.md) · Next: [3.12 Secrets, Key Management and Operational Security](12-secrets-and-key-management.md)

---

**1. Overall Objective**
Ship changes safely, repeatably and reversibly.

**2. The Problem We Are Solving**
Most production incidents are caused by deployments, especially schema
migrations on tables that can't be locked.

**3. Concepts and Research**
- Expand/contract migrations.
- Lock-safe DDL (`CREATE INDEX CONCURRENTLY`, `lock_timeout` in migrations).
- Immutable images.
- SBOM.
- Signed releases.
- Environment promotion (dev → staging → prod).

**4. Implementation Tasks**
- A multi-stage Dockerfile with a minimal image running as non-root.
- A CI pipeline:
  - fmt, clippy, unit, property, DB, concurrency, contract, `cargo deny`
  - the simulator smoke test
  - migration tests against a production-sized copy
- A migration linter that rejects unsafe DDL.
- A deploy script with health checks and rollback.
- Release notes template.

**5. Testing Requirements**
- Deploy N → N+1 → rollback to N under load: no errors and verify clean.
- An unsafe migration in a PR is blocked by CI.

**6. Deliverables**
Pipeline, images and deploy tooling.

**7. Acceptance Criteria**
A zero-downtime deploy is demonstrated under load.

**8. Dependencies**
Step [3.04](../phase-3-scale-and-integration/04-partitioning-and-archival.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
A staging environment runs continuously.
