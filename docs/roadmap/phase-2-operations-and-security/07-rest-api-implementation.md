# 2.07 REST API Implementation (Axum)

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.06 REST API Design (Contract First)](06-rest-api-design.md) · Next: [2.08 Authentication, Authorization and Privilege Separation](08-authentication-and-authorization.md)

---

**1. Overall Objective**
Implement the API exactly as specified.

**2. The Problem We Are Solving**
The HTTP layer is where timeouts, request limits and leaked errors creep in.

**3. Concepts and Research**
- Axum extractors and middleware (tower).
- Request body limits.
- Timeouts; what a client timeout does *not* cancel in the DB.
- Graceful shutdown that finishes in-flight transactions.
- Request IDs.

**4. Implementation Tasks**
- `mezgeb-api` crate: handlers call the templates and engine.
- Middleware: request ID, body limit, timeout, and a structured access log without sensitive fields.
- Problem Details error mapping.
- Contract tests generated from the OpenAPI spec.

**5. Testing Requirements**
- Postman collection runs in CI against a containerised service.
- A client timeout followed by a retry gives a single entry.
- Malformed amount, negative amount, float amount and huge payload are all rejected with the right code.
- Graceful shutdown during load loses no acknowledged request.

**6. Deliverables**
Running API service and CI contract tests.

**7. Acceptance Criteria**
100% of the spec's endpoints pass contract tests.

**8. Dependencies**
Step [2.06](../phase-2-operations-and-security/06-rest-api-design.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The chaos test from step 1.15 re-runs through the API layer and comes out clean.
