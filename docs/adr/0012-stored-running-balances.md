# 0012. Balances are stored running totals, proven against the lines

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-10 |
| Answers | [Architecture §5.7](../architecture.md#57-calculating-and-verifying-balances-efficiently) |
| Implemented in | 1.08 (schema), 1.09 (posting), 1.13 (verification), 2.05 (periods and snapshots) |

## Context

Every payment needs the current balance of the accounts it touches, under
lock, in milliseconds. Summing all lines of an account is always correct,
but it gets slower with every entry. A busy merchant may have millions of
lines.

## Decision

Each account row stores **`debits_total` and `credits_total`**, updated in
the posting transaction while the row is locked (ADR-0007). Reading a
balance is O(1).

Each journal line also stores **the account's totals after that line**, so
the balance at any point in history can be read directly, and the chain can
be checked line by line.

The stored totals are continuously **re-proven**:

- A verification job checks that every account's totals equal the sum of
  its lines, and that global debits equal global credits (step 1.13).
- **Snapshots** at period close mean verification only scans lines since the
  last verified point.

> Storing a balance does not make it correct. It is correct only because it
> is updated atomically with the lines, under lock, and continuously checked
> against them.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Stored totals + totals on each line + verification (chosen)** | O(1) balance reads under lock. Historical balances without a scan. Errors are detected. | Every posting updates the account row, which serialises postings to a hot account (ADR-0007). More columns to keep consistent. |
| Derive balances with `SUM` on read | Always correct by construction. | O(n) per read. Unusable for authorisation at scale. |
| Materialized views | Fast reads. | Stale between refreshes, so they cannot be used to authorise a payment. |
| Balances in a cache (Redis) | Very fast reads. | Not in the posting transaction, so it can disagree with the journal. |

## Consequences

**Positive:**

- Authorisation reads the exact balance under the same lock that protects
  it.
- Statements and "balance on date X" are simple queries.

**Negative:**

- A bug in updating totals would make balances wrong. Verification exists to
  catch it, and a failure alerts someone.
- Totals on each line add storage per line.
