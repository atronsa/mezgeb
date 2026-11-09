# 3.08 Multi-Currency, Multi-Tenant and ISO 20022 Evaluation

[Phase 3: Scale, Operations and Client Integration](README.md) · Previous: [3.07 Merchant Settlement Automation](07-merchant-settlement.md) · Next: [3.09 Observability: Logs, Metrics, Dashboards, Alerts](09-observability.md)

---

**1. Overall Objective**
Decide what to build now and what to defer. Build only what the requirements
justify.

**2. The Problem We Are Solving**
Over-building adds risk. Under-designing forces a rewrite later.

**3. Concepts and Research**
- Multi-currency accounting: per-currency balancing, FX via liquidity accounts, revaluation.
- Multi-tenancy: a tenant column vs schema per tenant vs DB per tenant.
- ISO 20022 message families relevant to bank integration (pain, pacs, camt).

**4. Implementation Tasks**
- Three ADRs:
  - Expected: multi-currency is structurally supported but only ETB is enabled.
  - Expected: multi-tenancy is deferred, but `client_id` scoping already isolates clients.
  - Expected: ISO 20022 is deferred until a partner bank requires it.
- Verify the existing schema allows each later without migrating historical data. Add a guard test that rejects a second currency until it is enabled.

**5. Testing Requirements**
- A test that a USD entry is rejected while it isn't enabled.
- A test proving per-currency balancing already works when a second currency is enabled in a test database.

**6. Deliverables**
Three ADRs and the guard tests.

**7. Acceptance Criteria**
Each deferral has explicit triggers ("build when …").

**8. Dependencies**
Steps [1.04](../phase-1-core-engine/04-money-and-identifier-types.md), [1.06](../phase-1-core-engine/06-journal-entries-and-posting-rules.md).

**9. Estimated Difficulty**
Low–Medium.

**10. Exit Gate**
The ADRs are reviewed. *Spare capacity: use the rest of this week to pay down slippage.*
