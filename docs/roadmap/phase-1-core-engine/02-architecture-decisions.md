# 1.02 Architecture Decision Records and Repository Restructure

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.01 Accounting Foundations and Worked Examples](01-accounting-foundations.md) · Next: [1.03 Invariant Catalogue, Threat Model v0, Regulatory Applicability](03-invariants-threat-model-and-regulation.md)

---

**1. Overall Objective**
Turn [`architecture.md`](../../architecture.md) into binding ADRs and align the scaffold with them.

**2. The Problem We Are Solving**
Undocumented decisions get re-argued every month. Each one changed later costs a migration of real money data.

**3. Concepts and Research**
- ADR format (context, decision, alternatives, consequences).
- Modular monolith vs services.
- Multi-line journal entries vs transfer pairs (TigerBeetle's model).
- Holds as postings vs pending columns.
- REST vs gRPC.

**4. Implementation Tasks**
- Write `docs/adr/0001` to `0010`: service boundary and own DB; layering (engine/ops/client); multi-line entries; holds as postings; money as `i64` minor units; UUIDv7 IDs; READ COMMITTED + ordered row locks; idempotency in the same transaction; REST/Axum; the offline escrow model. Also 0011 (append-only history) and 0012 (stored running balances), so that every question in architecture §5 maps to an ADR.
- Delete `proto/` and update spec §9 to REST.
- Rework the crate list: `mezgeb-core`, `mezgeb-engine` (renamed from `store-pg`), `mezgeb-ops`, `mezgeb-offline`, `mezgeb-api`, `mezgeb-sim`, `mezgeb-cli`.
- Mark the scaffold's transfer types for replacement.

**5. Testing Requirements**
- The workspace still builds and passes clippy after the restructure.
- An ADR review checklist: does each ADR list at least 2 alternatives and its consequences?

**6. Deliverables**
12 ADRs ([`docs/adr/`](../../adr/README.md)) and a restructured workspace.

**7. Acceptance Criteria**
Every question in [architecture.md §5](../../architecture.md#5-the-ten-architectural-questions) maps to an ADR.

**8. Dependencies**
Step [1.01](../phase-1-core-engine/01-accounting-foundations.md).

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
For any table or crate, you can point to the ADR that justifies it.
