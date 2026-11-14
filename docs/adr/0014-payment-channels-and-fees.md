# 0014. Payment channels are metadata; fees are lines in the payment's own entry

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-14 |
| Answers | E07's and E08's open questions ([payments](../accounting/entries/payments.md)); the fee questions deferred from step 2.02 |
| Implemented in | 2.03 (transfers, merchant payments and fees) |

## Context

A customer can pay through several channels: a QR scan, a card tap, a phone
tap or a fingerprint at a merchant, a payment request from another customer,
or a P2P transfer in the app. The ledger has to record which channel was
used, for reports, disputes and limits. It also has to book the fee the
platform earns on a payment, which the merchant or the payer may pay, and
the fee must never post without its payment or the other way round.

## Decision

**1. The channel is metadata on the entry, never an account.** Once a
payment is authorised, the money moves the same way whatever the channel
(E07–E09 have the same lines). Merchant payments are booked by one template,
`merchant_payment.v1`, which requires the metadata key `channel` with one of
a fixed set of values. The product list of channels, and the template that
books each:

| Channel | `channel` value | Template | Worked entry |
|---|---|---|---|
| QR code at a merchant | `qr` | `merchant_payment.v1` | E07 |
| Card tap at a merchant | `card_tap` | `merchant_payment.v1` | E08 |
| Phone tap at a merchant | `phone_tap` | `merchant_payment.v1` | (as E08) |
| Fingerprint at a merchant | `fingerprint` | `merchant_payment.v1` | E09 |
| Payment request | (its own template) | `payment_request_settle.v1`, with the `payment_request` it pays | E10 |
| P2P transfer in the app | (its own template) | `p2p_transfer.v1` | E06 |

An offline payment (an offline card tap, for example) is a different flow,
booked against offline escrow (E16–E18, steps 2.10–2.14).

The allowed values are part of a template version. A new channel is added
as a new version (`merchant_payment.v2`), and v1 keeps its values forever.

**2. Fees are lines in the payment's own entry.** A payment with a fee is
one entry of up to three lines, so the fee and the payment post or fail
together:

- a **merchant-paid fee** (`merchant_fee`) comes out of the price: the
  merchant is credited the price minus the fee (E07–E09);
- a **payer-paid fee** (`payer_fee`) is added to the price: the payer is
  debited the price plus the fee;
- both fees together are credited to 4010 Payment fee revenue in one line,
  and a zero fee books no fee line;
- the fees together may not exceed the price, and every amount is in one
  currency.

**3. The client computes every fee; the ledger never rounds.** The client
application applies its fee schedule and rounding rule and sends the fee in
minor units. The template checks only that the fee is well formed. The
rounding rule is the client's, and the client documents it with its fee
schedule.

**4. No fees on deposits, withdrawals or returns in this version.** No
product charges them yet. 4020 Service fee revenue and 5020 Bank charges
stay reserved. A fee there will be a new template version with a fee line
(for example `withdrawal_request.v2`), never a separate fee entry.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Channel as metadata, one template for all merchant channels (chosen)** | One booking to review and test; the trial balance shows money, not channels; new channels need no new accounts | Reports by channel read the metadata, not account balances |
| One revenue or merchant account per channel | Channel totals straight from balances | The chart grows with every channel; the same money is split across accounts for a reason that is not an accounting fact |
| A separate fee entry after the payment | Simpler templates | The fee can fail on its own or post without its payment, leaving the books wrong; two entries to reverse on a refund |
| The ledger computes fees from a rate | Clients cannot get the fee wrong | Puts business policy and rounding in the money system (against ADR-0002); every fee change becomes a ledger release |

## Consequences

**Positive:**

- Every channel books through a template, and a payment and its fee are
  atomic.
- Fee revenue in the trial balance is exactly the sum of the fee lines.

**Negative:**

- A new channel needs a new template version.
- Clients must document and test their fee rounding.

**Makes out of date:** the fee and channel open questions in E07 and E08,
and the "decided with fees in step 2.03" notes in E01–E04.
