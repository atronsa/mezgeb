# Clearing and Suspense

[Accounting model](README.md) · [Chart of accounts](chart-of-accounts.md) · Decided in step [2.02](../roadmap/phase-2-operations-and-security/02-deposits-withdrawals-and-clearing.md)

Money that enters or leaves through the partner bank is not instant, and not
always identified. This page defines how the ledger books money while it is
in flight (**clearing**) and while its owner is unknown (**suspense**), so
that it is never invisible and never counted twice.

## The accounts

| Account | Type | Holds | Each item ends |
|---|---|---|---|
| `2020:wd-…` Pending withdrawals | Liability | A withdrawal the customer asked for, not yet sent to the bank | Sent (to 9030) |
| `9030:wdt-…` Clearing: withdrawals in transit | Liability | A payout submitted to the bank, not yet confirmed or rejected | Confirmed (to 1010) or rejected (back to 2010) |
| `9010:sus-…` Suspense: unidentified funds | Liability | Money in the bank whose owner is not known yet | Resolved to its owner (2010 or 2030) |
| `9040:sbd-…` Suspense: unexplained bank debits | Asset | Money the ledger shows in 1010 that the bank does not, parked by reconciliation (step 2.15) | Explained: back to 1010, to a wallet or merchant balance, or to 5020 |
| 9020 Clearing: deposits in transit | Asset | Reserved, unused | |

All four are **control accounts with one sub-account per item**: one per
withdrawal, one per payout in transit, one per unidentified payment, one
per unexplained bank debit. The
client application chooses the sub-account ID (for example, the withdrawal's
own ID), opens it, and passes it to the template.

## Rules

1. **Book only what the bank shows.** 1010 must always equal the bank
   statement. Cash is debited to 1010 when the statement shows it arriving
   (E01, E19) and credited when the statement shows it leaving (E03 (b)),
   never on an earlier notice. This is why deposits need no clearing account
   today: the notice the platform books on is already final. 9020 stays
   reserved until a bank integration gives a notice that can still be undone.
2. **Every item has its own sub-account, and it returns to zero.** A
   template that opens an item refuses a sub-account that has already been
   posted to (template error 2015). An item is open while its balance is not
   zero, and finished when it is back at zero.
3. **Items move whole.** Sending, confirming, rejecting and resolving move
   the item's entire balance, and the template refuses any other amount
   (2014). A rejected withdrawal therefore returns exactly the amount that
   was requested.
4. **Each item has exactly one outcome.** None of these accounts may go below
   zero (INV-8). Once an item is back at zero, a second outcome, such as a
   confirmation after a rejection, a rejection after a confirmation, or the
   same bank notice twice, is refused by the posting rules (1012). The
   template's own check (rule 3) refuses it first, but the posting rule is
   the guarantee: it is checked again inside the posting transaction, with
   the accounts locked.
5. **Suspense is never revenue.** Unidentified money is owed to someone. It
   is resolved to its owner or, if none is found, returned to the sender. It
   is never moved to the platform's own accounts.
6. **These balances are customer money.** 2020, 9030 and 9010 are backed by
   1010 in the safeguarding check, like the wallets themselves.

## Lifecycles and templates

Each step is one posting template (`crates/ops`). The worked entry shows its
lines.

**Deposit**

| Event | Template | Lines | Entry |
|---|---|---|---|
| The bank statement shows a deposit for a known customer | `deposit_confirmed.v1` | Dr 1010 / Cr `2010:cust-…` | E01 (also E05: a returned payout) |
| The bank statement shows a deposit whose owner is unknown | `deposit_unmatched.v1` | Dr 1010 / Cr `9010:sus-…` (new) | E19 |
| The owner is found | `suspense_resolve.v1` | Dr `9010:sus-…` / Cr `2010:cust-…` or `2030:merch-…`, the whole balance | E20 |

**Withdrawal**

```
requested ──► pending (2020) ──sent──► in transit (9030) ──confirmed──► paid (1010)
                                                 └──────rejected──────► back to the wallet (2010)
```

| Event | Template | Lines | Entry |
|---|---|---|---|
| The customer asks for a payout | `withdrawal_request.v1` | Dr `2010:cust-…` / Cr `2020:wd-…` (new) | E02 |
| The payout is submitted to the bank | `withdrawal_sent.v1` | Dr `2020:wd-…` / Cr `9030:wdt-…` (new), the whole balance | E03 (a) |
| The bank confirms it | `withdrawal_confirmed.v1` | Dr `9030:wdt-…` / Cr 1010, the whole balance | E03 (b) |
| The bank rejects it | `withdrawal_failed.v1` | Dr `9030:wdt-…` / Cr `2010:cust-…`, the whole balance | E04 |

A withdrawal is booked with ordinary entries, not a ledger hold (step 1.14).
A hold expires by itself and gives the money back, which is wrong once a
payout may already be at the bank: the customer would be paid twice.
Instead, a withdrawal that stays pending or in transit too long shows up in
the ageing report, and a person decides.

**Reconciliation** (step 2.15, [ADR-0020](../adr/0020-reconciliation.md))

When the bank statement and 1010 disagree, the first entry makes 1010 equal
the bank again and parks the difference; the second closes it once the
cause is found ([E32–E35](entries/reconciliation.md)).

| Event | Template | Lines | Entry |
|---|---|---|---|
| The bank shows more than the ledger | `deposit_unmatched.v1` | Dr 1010 / Cr `9010:sus-…` (new) | E19 |
| The bank shows less than the ledger | `bank_debit_unmatched.v1` | Dr `9040:sbd-…` (new) / Cr 1010 | E32, E34 |
| The cause of a bank debit is found | `bank_debit_resolve.v1` | Dr 1010, `2010:cust-…`, `2030:merch-…` or 5020 / Cr `9040:sbd-…`, the whole balance | E33, E35 |

9040 is an asset, but not cash: it never backs customer money. While an
item is open, the safeguarding check shows the shortfall in 1010.

**Clearing nets to zero.** At any time, the balance of 9030 is the sum of
the payouts submitted and not yet confirmed or rejected, and 2020 the sum of
the withdrawals requested and not yet sent. Each finished lifecycle leaves
both at zero.

## Ageing

The ageing report lists every open item in 2020, 9030, 9010 and 9040 (and
2060, disputed merchant funds, step 2.04), oldest first, with its balance
and how long it has been open (since its first posting):

| Bucket | Pending withdrawal (2020) | In transit (9030) | Suspense (9010) | Bank debit (9040) |
|---|---|---|---|---|
| Under 1 day | Normal: waits for the next batch | Normal: waits for the bank | Normal: being matched | Normal: being explained |
| 1 to 3 days | Late: the batch missed it | Late: chase the bank | Investigate | Chase the bank or the client |
| 3 to 7 days | Alert | Alert | Alert | Alert |
| 7 days and over | Escalate | Escalate | Escalate; consider returning to the sender | Escalate; report the shortfall |

The query is `mezgeb_engine::ageing`, called with the account codes in
`mezgeb_ops::AGED_ACCOUNTS`. The engine does not know what the codes mean;
this page does.

## Not decided yet

- **Fees** on deposits, withdrawals and returns (4020): none in this
  version ([ADR-0014](../adr/0014-payment-channels-and-fees.md)). A fee will
  be a new template version with a fee line. A charge the bank takes from
  1010 is found by reconciliation and booked to 5020 through 9040 (E32,
  E33).
- **Cancelling a withdrawal before it is sent** (Dr 2020 / Cr 2010) and
  **returning unmatched money to the sender** (Dr 9010 / Cr 1010): no
  template yet.
- **Thresholds per client.** The buckets above are a starting point; an
  operator may want different alert times per bank.
