# 0005. Money is an `i64` count of minor units

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-10 |
| Answers | [Architecture §4](../architecture.md#4-data-model-at-a-glance) |
| Implemented in | 1.04 (money and currency types), 1.08 (`BIGINT` columns) |

## Context

Money must be exact. Binary floating point cannot store 0.10 or 39.20
exactly, so sums drift over millions of entries (see E12). The ledger also
must never round silently (ADR-0002).

Every currency has a smallest unit defined by ISO 4217: the santim for ETB
(2 decimals), the cent for USD (2 decimals), the yen for JPY (0 decimals).

## Decision

An amount is a **signed 64-bit integer count of the currency's minor unit**,
paired with its currency. In PostgreSQL it is a `BIGINT`.

- Journal line amounts must be **greater than zero**. The side (debit or
  credit) gives the direction, never the sign.
- All arithmetic is **checked**. Overflow is an error, never a wrap.
- The core has no floats and no decimal types. Conversion to and from a
  decimal string (for example, `"39.20"`) happens only at the API boundary,
  using the currency's exponent, and rejects extra decimals instead of
  rounding.
- Amounts in different currencies can never be added together. The type
  system prevents it.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **`i64` minor units (chosen)** | Exact. Fast native arithmetic. Range is about 9.2 × 10¹⁸ santim (92 quadrillion ETB), far beyond any account. Maps to `BIGINT`. | Every input and output must convert using the currency exponent. A currency with many decimals would reduce the range. |
| `f64` | Convenient. | Not exact. Ruled out for money. |
| Arbitrary-precision decimal (`NUMERIC`, `rust_decimal`) | Exact, and allows sub-minor-unit values. | Slower, larger, and makes silent rounding possible inside the core. Values smaller than a santim cannot be paid out anyway. |
| `i128` minor units | Practically unlimited range. | No native PostgreSQL type of that size. The extra range is not needed. |

## Consequences

**Positive:**

- E12 is 3,920 + 80 = 4,000 santim, with no rounding anywhere.
- Sums and balance checks are simple integer operations.

**Negative:**

- Fees and exchange rates that produce fractions must be rounded by the
  client before calling the ledger, under a written rule.
- Running totals are also `i64` and must use checked arithmetic.

**Makes out of date:** nothing. This matches spec §3.2.
