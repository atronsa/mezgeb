# 1.11 Idempotency and Exactly-Once Effect

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.10 Concurrency Control and Concurrent Spending](10-concurrency-control.md) · Next: [1.12 Reversals and Adjustments](12-reversals-and-adjustments.md)

---

**1. Overall Objective**
Make retries safe: the same request has the same effect once.

**2. The Problem We Are Solving**
Networks drop responses. Clients retry. Without idempotency, users get charged twice.

**3. Concepts and Research**
- Idempotency keys (the IETF draft on the `Idempotency-Key` header).
- At-least-once delivery combined with idempotent receivers gives an exactly-once effect.
- Request fingerprinting.
- Key scope and retention.
- Concurrent duplicates.

**4. Implementation Tasks**
- An idempotency record in the same transaction as the entry (`client_id`, key, request hash, entry id, response snapshot, created_at).
- Canonical request serialisation for the hash.
- Concurrent same-key requests: the second waits on the unique index then returns the stored result.
- Same key with a different hash returns an `IDEMPOTENCY_KEY_REUSED` error.
- `lookup_by_idempotency_key`.
- A retention policy (the key lives at least as long as the client's retry window; document it).

**5. Testing Requirements**
- The same request 10 times sequentially gives 1 entry.
- The same request 50 times concurrently gives 1 entry and 50 identical responses.
- The same key with a different amount gives an error and no entry.
- Failure injection: abort after commit but before the response (the test hook drops the connection), then retry: the original result returns and no new entry exists.

**6. Deliverables**
Idempotency module and tests.

**7. Acceptance Criteria**
None of the tests can produce a duplicate entry.

**8. Dependencies**
Steps [1.09](../phase-1-core-engine/09-atomic-posting.md)–[1.10](../phase-1-core-engine/10-concurrency-control.md).

**9. Estimated Difficulty**
High.

**10. Exit Gate**
A crash-after-commit retry is demonstrated in a recorded test run.
