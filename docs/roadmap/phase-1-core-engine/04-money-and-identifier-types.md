# 1.04 Money, Currency and Identifier Types

[Phase 1: Accounting Model and Core Engine](README.md) · Previous: [1.03 Invariant Catalogue, Threat Model v0, Regulatory Applicability](03-invariants-threat-model-and-regulation.md) · Next: [1.05 Chart of Accounts and the Account Model](05-chart-of-accounts.md)

---

**1. Overall Objective**
Make invalid money values impossible to represent in Rust.

**2. The Problem We Are Solving**
Float rounding, silent overflow, mixed currencies and swapped IDs all corrupt balances quietly.

**3. Concepts and Research**
- Minor units and the ISO 4217 exponent.
- Why not `f64`; `i64` vs `NUMERIC` (ADR-0005).
- Checked arithmetic.
- The newtype pattern.
- UUIDv7 ordering.
- JSON number precision: integers above 2^53 lose precision in JavaScript, so the API sends amounts as strings.

**4. Implementation Tasks**
In `mezgeb-core`:
- `Currency` (code and exponent; a static ISO 4217 table, ETB first).
- `Money { amount: i64, currency }` with only checked operations; adding different currencies is an error.
- `PositiveAmount`.
- Newtypes: `AccountId`, `EntryId`, `HoldId`, `ClientId`, `IdempotencyKey` (length and charset validated).
- Parsing "123.45" ↔ minor units with exact rules: too many decimals → reject, never round.
- Serde implementations that use strings.

**5. Testing Requirements**
- Property tests: parse(format(x)) == x; checked add/sub never panics and errors on overflow; cross-currency operations always error.
- Unit tests: "0.001" ETB is rejected, "-0" is rejected, max/min i64 boundaries behave.

**6. Deliverables**
`money.rs`, `currency.rs`, `ids.rs` with tests.

**7. Acceptance Criteria**
100% branch coverage on the money module, no `unwrap`, and clippy clean.

**8. Dependencies**
Step [1.02](../phase-1-core-engine/02-architecture-decisions.md) (ADR-0005, 0006).

**9. Estimated Difficulty**
Low–Medium.

**10. Exit Gate**
Property tests pass for 1M cases.
