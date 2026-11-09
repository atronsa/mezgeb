# 2.01 Operations Layer and Posting Templates

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [1.16 Phase 1 Consolidation, Baseline Benchmark and Exit Review](../phase-1-core-engine/16-phase-1-exit-review.md) · Next: [2.02 Deposits, Withdrawals, Clearing and Suspense Accounts](02-deposits-withdrawals-and-clearing.md)

---

**1. Overall Objective**
Separate *how a business event is booked* from the engine, using versioned
posting templates.

**2. The Problem We Are Solving**
If clients build raw lines themselves, every client re-implements accounting
rules and gets some of them wrong.

**3. Concepts and Research**
- Posting rules (accounting policy as code).
- Template versioning: old entries keep their template version forever.
- The boundary between "is it allowed?" (client) and "how is it booked?"
  (template), from ADR-0002.

**4. Implementation Tasks**
- `mezgeb-ops` crate with a `Template` trait: typed request → `EntryDraft`.
- `TemplateRegistry` keyed by `name.vN`; entries store template name and version.
- A rule that a template may only reference accounts by role (e.g. `payer_wallet`, `fee_revenue`), which the request resolves.
- First template: `p2p_transfer.v1`.

**5. Testing Requirements**
- Each template is tested against its step 1.01 worked example.
- Property test: any valid request yields an entry that `validate` accepts.
- Unknown template or version → stable error.

**6. Deliverables**
`mezgeb-ops` and the template registry.

**7. Acceptance Criteria**
The engine has no knowledge of any template name.

**8. Dependencies**
Phase 1.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
A new template can be added without touching the engine.
