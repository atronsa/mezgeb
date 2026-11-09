# 3.06 Transactional Outbox and Event Integration

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.05 Reporting and Financial Statements](05-reporting-and-financial-statements.md) · Next: [3.07 Merchant Settlement Automation](07-merchant-settlement.md)

---

**1. Overall Objective**
Notify clients reliably of ledger events (posted, hold expired, receipt
settled, conflict) with no message broker.

**2. The Problem We Are Solving**
"Write to the DB, then publish" loses or duplicates events when a crash falls
between the two steps.

**3. Concepts and Research**
- Transactional outbox.
- At-least-once delivery.
- Consumer idempotency.
- Ordering guarantees per account.
- Webhooks vs polling (clients pull via a cursor API, the simpler and safer option).

**4. Implementation Tasks**
- `outbox` table written in the posting transaction.
- `GET /v1/events?after=cursor` with ordered, paginated events.
- Optional signed webhooks later.
- A retention job.

**5. Testing Requirements**
- Crash between commit and delivery: the event is still delivered.
- Duplicate delivery is safe on the consumer side (test with a client stub).
- Ordering per account is preserved.

**6. Deliverables**
Outbox and events API.

**7. Acceptance Criteria**
In a 1M-event chaos run, zero events are lost.

**8. Dependencies**
Step [2.07](../phase-2-operations-and-security/07-rest-api-implementation.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The broker decision is recorded: still not needed, with the reason.
