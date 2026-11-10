# 0004. Holds are real postings to held sub-accounts

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-10 |
| Answers | [Architecture §4](../architecture.md#4-data-model-at-a-glance) |
| Implemented in | 1.14 (holds and two-phase operations) |

## Context

Some money must stop being spendable before it finally moves: a withdrawal
waiting for the bank (E02), a card payment authorised now and captured later,
an offline allowance (E16). The amount still belongs to the customer, but
spending it twice must be impossible.

There are two common ways to model this. One adds *pending* columns next to
the balance. The other books the held amount into a separate account.

## Decision

A hold is **a real journal entry** that moves the amount from the account
into a held sub-account (`…:held`), plus a **hold record** that tracks its
status, expiry and the entry that finally captures or releases it.

- **Capture:** an entry from the held sub-account to the destination.
- **Release or expiry:** an entry from the held sub-account back to the
  account.
- A hold resolves **at most once**: captured, released or expired.
- A capture may be for less than the held amount. The remainder is
  released in the same operation.

The worked entries already use this pattern with dedicated accounts:
pending withdrawals (2020), pending settlements (2040) and offline escrow
(2050).

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Holds as postings (chosen)** | Held money is visible in the trial balance and in every report. One mechanism (entries) for all money movement. Every pending amount can be listed, totalled and aged. | More entries and more accounts. The available balance is the account's balance, while the held amount sits in another account. |
| Pending debit and credit columns on the account | Fewer rows. The available balance and the pending amount are on one row. | Pending money is invisible to the journal and the trial balance. Two sets of totals to keep consistent. Expiry changes balances without an entry. |
| Holds tracked only in the client | Simplest ledger. | The ledger cannot stop a double spend, which defeats its purpose. |

## Consequences

**Positive:**

- Every movement of money, including a reservation, is an entry with an
  audit trail.
- A hold that never resolves shows up as a balance that should be zero.

**Negative:**

- A background job must expire holds. It must be idempotent and resumable
  (ADR-0008).
- Clients must understand that "available" excludes held sub-accounts.

**Makes out of date:** spec §3.4 pending totals, §3.5 `PENDING`,
`POST_PENDING` and `VOID_PENDING` flags, and §4.3 two-phase transfers. They
are rewritten in step 1.14.
