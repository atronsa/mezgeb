# Request: External Security Review of the Offline Protocol v1

[Documentation](../README.md) · Protocol: [protocol.md](protocol.md) · Threats: [threat-addendum.md](threat-addendum.md) · Scenarios: [scenarios.md](scenarios.md)

| | |
|---|---|
| Requested | 2026-11-16, at the end of step 2.10 |
| Reviewer | A security engineer with a cryptography background, independent of the project ([roadmap, specialist review](../roadmap/README.md#specialist-review-needed)) |
| Needed before | Step 2.11 builds device registration, and before any card is personalised |
| Status | **Requested: no reviewer engaged yet** |

## What to review

[protocol.md](protocol.md) (Mezgeb offline protocol v1): an escrow-backed
offline payment scheme. A payer's device, a smart card or a phone, signs
receipts against a voucher the ledger issued online. The ledger later
settles those receipts from escrow, exactly once, whoever uploads them.

Background: [ADR-0010](../adr/0010-offline-escrow-model.md) (the escrow
model) and [ADR-0019](../adr/0019-offline-protocol-v1.md) (the v1
decisions).

## Questions for the reviewer

1. **Encoding and signatures** (§2–3). Is the tagged canonical encoding
   unambiguous? Is ECDSA P-256 with low-S and a digest that excludes the
   signature enough to rule out malleability? Should RFC 6979 be required,
   not only recommended?
2. **Keys** (§4). Is the root → issuer → device hierarchy sound? Are 90-day
   issuer rotation and the revocation lists adequate, given that terminals
   are often offline?
3. **Attestation** (§5). Is the tier assignment from Android Key Attestation
   and card chip certificates correct and complete? What can a rooted phone
   still fake?
4. **Phones without hardware counters** (§6, §10, OT-02). Is detection after
   the fact, bounded by tier limits, an acceptable control? Is there a better
   option on Android (for example, Protected Confirmation, or StrongBox
   rollback resistance)?
5. **Replay and freshness** (§8, OT-03). Does `payee_challenge`, with the
   payee bound inside the signed body, close every replay path?
6. **Gaps and conflicts** (§9–11). Can a 72-hour skipped gap be abused to
   settle a receipt that would have been a conflict? Is matching a late
   missing receipt against its successor's `prev_digest` enough?
7. **The card requirements** (§15, H1–H8). Are they complete for a contactless
   card that signs payments offline?
8. **The threat addendum.** What is missing, or rated too optimistically?

## What we ask back

- Findings, rated by severity, with the protocol section they concern.
- For each high-severity finding, a recommended change.
- An overall opinion: build as specified, build after named changes, or
  redesign named parts.

Findings are recorded under `docs/offline/` and answered before step 2.11
proceeds past device registration. A protocol change is a new ADR and a new
protocol version.
