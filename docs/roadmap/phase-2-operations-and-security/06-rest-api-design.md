# 2.06 REST API Design (Contract First)

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.05 Accounting Periods, Closing and Trial Balance](05-accounting-periods-and-closing.md) · Next: [2.07 REST API Implementation (Axum)](07-rest-api-implementation.md)

---

**1. Overall Objective**
Design the public API from the now-stable model before implementing it.

**2. The Problem We Are Solving**
An API is a long-term promise. Mistakes become permanent for every client.

**3. Concepts and Research**
- Resource design for ledgers (accounts, entries, holds, operations).
- The `Idempotency-Key` header.
- RFC 9457 Problem Details.
- Cursor pagination.
- Amounts as strings.
- Versioning (`/v1`).
- OWASP API Security Top 10.
- OpenAPI 3.1.

**4. Implementation Tasks**
- Write `openapi/mezgeb.v1.yaml`. Endpoints:
  - accounts: create and get
  - balance at a point in time
  - entry history
  - posting operations (one per template)
  - reversal
  - holds
  - lookup by idempotency key
  - periods
  - reports
  - offline endpoints, stubbed
- An error code catalogue mapped to `PostingError`.
- A Postman collection generated from the spec.

**5. Testing Requirements**
- Spec lint.
- A review of every endpoint against OWASP API Top 10.
- Examples for each error code.

**6. Deliverables**
OpenAPI spec, error catalogue and Postman collection.

**7. Acceptance Criteria**
Every template and engine operation is reachable, and no endpoint allows raw arbitrary lines except an admin-only adjustment.

**8. Dependencies**
Steps [2.01](../phase-2-operations-and-security/01-posting-templates.md)–[2.05](../phase-2-operations-and-security/05-accounting-periods-and-closing.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The spec is reviewed and frozen at v1-draft.
