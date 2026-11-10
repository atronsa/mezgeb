# 0002. Three layers: engine, posting templates, client application

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-10 |
| Answers | [Architecture §5.2](../architecture.md#52-separating-the-engine-from-wallet-and-payment-logic), [§5.9](../architecture.md#59-reconciling-cached-balances-against-the-ledger) |
| Implemented in | 1.09 (engine), 2.01 (posting templates), 2.02–2.04 (flows), 2.15 (reconciliation) |

## Context

A wallet platform mixes two kinds of rules:

- **Accounting rules** that are always true: entries balance, history is
  never edited, an account may not go below zero unless allowed.
- **Business rules** that change often: fee rates, limits, KYC levels,
  fraud checks, which customers may pay which merchants.

If the two are mixed in one layer, every change to a fee or limit risks
breaking the accounting, and the ledger cannot be reused by a platform with
different business rules.

## Decision

Three layers, each depending only on the one below it:

1. **Engine.** Knows accounts, entries, lines, holds, reversals and periods.
   Enforces accounting rules and nothing else.
2. **Posting templates.** Named, versioned functions such as
   `p2p_transfer.v1` or `merchant_payment.v1` that turn a typed request into
   a balanced entry. They define *how* a business event is booked, never
   *whether* it is allowed.
3. **Client application.** Decides whether an operation is allowed (KYC,
   limits, fraud rules), computes fee amounts, then calls a template.

The ledger never rounds. Fee amounts arrive already computed, and the
template only checks that the entry is well formed.

**The ledger is the system of record.** Any balance stored outside it, such
as a wallet balance table in the client, is a display cache. A recurring
reconciliation compares the cache with the ledger. If they differ, the
**ledger wins**: the cache is refreshed and an alert is raised. Decisions to
authorise a payment read the ledger, never the cache.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Engine, templates, client (chosen)** | Accounting rules are tested once and cannot be broken by a business change. Templates make each flow's booking explicit and reviewable. The ledger is reusable. | One more layer to learn. A new flow needs a new template version. |
| Engine only, client builds raw entries | Maximum flexibility for clients. | Every client re-implements bookings and can get them wrong. No reviewable list of how flows are booked. |
| Business logic inside the ledger (fees, limits, KYC) | One place for everything. | Ties the ledger to one platform's policies. Every policy change becomes a change to the money system. |

## Consequences

**Positive:**

- The worked entries E01–E18 map directly to templates, which makes them
  testable against the accounting review.
- Fee and limit changes never require a ledger release.

**Negative:**

- The client must compute fees and must not round silently. The rounding
  rule is the client's responsibility and is documented with the template.
- Template versions must be kept forever, because old entries reference
  the version that created them.
