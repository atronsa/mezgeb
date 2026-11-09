# 2.08 Authentication, Authorization and Privilege Separation

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.07 REST API Implementation (Axum)](07-rest-api-implementation.md) · Next: [2.09 Audit Logging and Tamper Evidence](09-audit-logging.md)

---

**1. Overall Objective**
Make sure only the right caller can perform each financial operation.

**2. The Problem We Are Solving**
The ledger's biggest risk is a legitimate-looking request from the wrong
party, such as a compromised client or a rogue admin.

**3. Concepts and Research**
- Service-to-service authentication: mTLS vs signed tokens vs hashed API keys (ADR).
- RBAC roles: `poster`, `reader`, `operator`, `approver`, `auditor`, `admin`.
- Least privilege.
- Separation of duties.
- Per-client scoping (a client can only touch its own accounts).
- OWASP ASVS V4 (access control).

**4. Implementation Tasks**
- `clients` and `client_credentials` tables, with credentials hashed (Argon2) or certificates pinned.
- A permission matrix as data: role × operation × template.
- An authorization middleware with **default deny**.
- Maker-checker for adjustments and manual period reopen.
- Per-client rate limits.

**5. Testing Requirements**
Security tests:
- Every endpoint × every role matches the permission matrix; anything not explicitly allowed is denied.
- Client A cannot read or post against client B's accounts.
- Replaying a revoked credential fails.
- An approver cannot approve their own request.

**6. Deliverables**
Authn/authz module, the permission matrix and the authorization test suite.

**7. Acceptance Criteria**
The auto-generated role × endpoint test matrix passes, and no endpoint lacks an authorization test.

**8. Dependencies**
Step [2.07](../phase-2-operations-and-security/07-rest-api-implementation.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The threat model's "compromised client" and "insider" threats are each mapped to a passing test.
