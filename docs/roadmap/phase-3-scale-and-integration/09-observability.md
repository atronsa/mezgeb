# 3.09 Observability: Logs, Metrics, Dashboards, Alerts

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.08 Multi-Currency, Multi-Tenant and ISO 20022 Evaluation](08-multi-currency-and-multi-tenant.md) · Next: [3.10 Backups, Point-in-Time Recovery and Disaster Recovery](10-backups-and-disaster-recovery.md)

---

**1. Overall Objective**
See the ledger's health and financial integrity in real time.

**2. The Problem We Are Solving**
Silent failures, such as a stalled expiry worker, rising suspense or a failed
verification, cost money when nobody notices them.

**3. Concepts and Research**
- Structured logging (`tracing`, JSON).
- The RED/USE methods.
- Prometheus metric types and cardinality limits.
- Alert design: actionable alerts only.
- Distributed tracing: **not justified** with one service. Request IDs propagated from the client are enough; revisit if more services appear.

**4. Implementation Tasks**
- A `/metrics` endpoint.
- Technical metrics: latency, errors, lock waits, retries, pool saturation.
- Financial metrics:
  - postings by template
  - suspense balance
  - clearing ageing
  - escrow exposure
  - unresolved conflicts
  - verify result
  - checkpoint age
- Grafana dashboards as code.
- Alert rules: verify failure, checkpoint missing, suspense above threshold, a stalled worker, error rate.

**5. Testing Requirements**
- Fire every alert in staging by inducing its condition.
- No PII or secrets in logs (automated log scan test).

**6. Deliverables**
Metrics, dashboards and alert rules.

**7. Acceptance Criteria**
Every alert has a runbook link and was test-fired.

**8. Dependencies**
Steps [1.13](../phase-1-core-engine/13-balance-verification.md), [2.07](../phase-2-operations-and-security/07-rest-api-implementation.md), [2.13](../phase-2-operations-and-security/13-offline-conflicts-and-expiry.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
A deliberately stalled worker is detected by the alerts within the agreed time.
