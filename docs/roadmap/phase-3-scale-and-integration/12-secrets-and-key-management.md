# 3.12 Secrets, Key Management and Operational Security

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.11 Deployment and CI/CD](11-deployment-and-ci-cd.md) · Next: [3.13 Client Integration Design and Account Mapping](13-client-integration-design.md)

---

**1. Overall Objective**
Protect the keys and credentials whose compromise means direct financial
loss.

**2. The Problem We Are Solving**
A leaked voucher-signing key lets an attacker mint offline spending. Leaked
DB credentials bypass the API entirely.

**3. Concepts and Research**
- Secrets managers vs environment files.
- Key rotation without downtime (key IDs in vouchers).
- HSM/KMS evaluation for the issuer key.
- Postgres hardening: TLS, `scram-sha-256`, `pg_hba`, no superuser for the app, pgaudit evaluation.
- Network exposure.

**4. Implementation Tasks**
- Move all secrets to a manager.
- Key IDs on vouchers and checkpoints.
- Rotation procedures for the issuer key, checkpoint key, API credentials and DB passwords.
- DB hardening and a review of role grants.
- An HSM/KMS decision ADR.

**5. Testing Requirements**
- Security tests: a rotated issuer key — old vouchers stay valid until expiry and new ones use the new key.
- A revoked key is rejected.
- The app role cannot `ALTER`, `TRUNCATE` or disable triggers (attempt each).
- A secret scan of the repo and images comes back clean.

**6. Deliverables**
Key management and hardening, plus rotation runbooks.

**7. Acceptance Criteria**
Every secret has an owner, a location, a rotation procedure and a tested rotation.

**8. Dependencies**
Steps [2.08](../phase-2-operations-and-security/08-authentication-and-authorization.md), [2.11](../phase-2-operations-and-security/11-device-registry-and-vouchers.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The threat model's key-compromise scenarios each have a tested response.
