# Phase 2: Financial Operations, API, Security and Offline (Weeks 17–32)

**Goal:** real money operations as versioned posting templates, a secured REST
API, tamper-evident audit, offline escrow settlement and reconciliation.

---

## Week 17: Operations Layer and Posting Templates

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
- Each template is tested against its Week 1 worked example.
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

---

## Week 18: Deposits, Withdrawals, Clearing and Suspense Accounts

**1. Overall Objective**
Book money entering and leaving the closed loop, including the uncertain
period while an external party confirms.

**2. The Problem We Are Solving**
External money is asynchronous and fallible. Without clearing accounts, money
"in flight" is either invisible or double counted.

**3. Concepts and Research**
- Clearing accounts (in flight) vs suspense accounts (unidentified or unresolved).
- Float / settlement bank asset account.
- Withdrawal lifecycle: requested → hold → sent → confirmed | failed → released.
- Deposits that cannot be matched to a user.

**4. Implementation Tasks**
- Templates: `deposit_confirmed.v1`, `deposit_unmatched.v1` (to suspense), `suspense_resolve.v1`, `withdrawal_request.v1` (hold), `withdrawal_sent.v1` (held → clearing), `withdrawal_confirmed.v1` (clearing → bank asset), `withdrawal_failed.v1` (clearing → wallet).
- An ageing query for suspense and clearing balances.

**5. Testing Requirements**
- Full happy path and every failure path; balances are checked at each step.
- A failed withdrawal returns exactly the original amount.
- Confirm-after-fail and fail-after-confirm are both rejected.
- Clearing nets to zero after every completed lifecycle.

**6. Deliverables**
Seven templates and the ageing report.

**7. Acceptance Criteria**
After 10k random deposit/withdrawal lifecycles, clearing = SUM(in-flight withdrawals).

**8. Dependencies**
Weeks 14, 17.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The suspense and clearing account policies are documented in `accounting-model.md`.

---

## Week 19: Transfers, Merchant Payments and Fees

**1. Overall Objective**
Book P2P transfers and merchant payments from every channel (QR, card tap,
phone tap, fingerprint), with fees in the same entry.

**2. The Problem We Are Solving**
A payment and its fee must succeed or fail together. A separate fee entry can
fail on its own and leave the books wrong.

**3. Concepts and Research**
- Gross vs net settlement to the merchant.
- Payer-paid vs merchant-paid fees.
- Fee revenue as revenue.
- Channel recorded as metadata, not as separate accounts (decide in an ADR).
- Fee calculation stays with the client; the ledger never rounds.

**4. Implementation Tasks**
- Templates: `merchant_payment.v1` (payer → merchant, optional fee lines to fee revenue) and `payment_request_settle.v1`.
- A `channel` metadata field with an enumerated set.
- Validate that fee ≤ amount and that all lines share one currency.

**5. Testing Requirements**
- Payer-paid and merchant-paid fee variants.
- A zero fee produces no fee line.
- Fee > amount is rejected.
- Concurrent payments to one merchant from 100 payers: merchant total = sum of the net amounts.

**6. Deliverables**
Payment templates.

**7. Acceptance Criteria**
Fee revenue in the trial balance equals the sum of the fee lines.

**8. Dependencies**
Week 17.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
Every channel in the product list books through a template.

---

## Week 20: Refunds and Disputes

**1. Overall Objective**
Support full and partial refunds tied to the original payment, and a dispute
flow for the closed loop.

**2. The Problem We Are Solving**
Refunding more than was paid is a direct theft path. So is refunding twice.

**3. Concepts and Research**
- Refund vs reversal (ADR).
- Cumulative refund cap.
- Fee policy on refund: whether the fee is returned (business decision, configured per template).
- Dispute lifecycle: opened → merchant funds held → resolved for payer or for merchant.

**4. Implementation Tasks**
- `refund.v1` references `original_entry_id`. The engine locks the original's refund tally row and enforces `sum(refunds) ≤ original net amount`.
- `refund_tallies` table, updated in the same transaction.
- `dispute_open.v1` (a hold on the merchant), `dispute_resolve_payer.v1`, `dispute_resolve_merchant.v1`.

**5. Testing Requirements**
- Partial refunds up to the cap succeed; one more santim is rejected.
- 50 concurrent partial refunds on one payment: the total never exceeds the cap.
- Refund after reversal: rejected.
- Dispute when the merchant's balance is insufficient: defined behaviour (reject, or hold what is available plus suspense; document it).

**6. Deliverables**
Refund and dispute templates and the tally table.

**7. Acceptance Criteria**
The over-refund property test passes 10k runs.

**8. Dependencies**
Weeks 12, 14, 19.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The dispute policy is written down and reviewed.

---

## Week 21: Accounting Periods, Closing and Trial Balance

**1. Overall Objective**
Add accounting periods, a close procedure, period snapshots and trial balance
reports.

**2. The Problem We Are Solving**
Without closed periods, reports change after they were issued and audits
cannot rely on them.

**3. Concepts and Research**
- Periods (daily operational, monthly accounting).
- Open → closing → closed.
- Effective date vs posting date.
- Late postings go to the current open period with a reference to the original date.
- Balance snapshots.
- How production ledgers implement a daily close.

**4. Implementation Tasks**
- `periods` and `period_snapshots` tables.
- `close_period`:
  1. Block new postings dated in it.
  2. Run verify for the period.
  3. Write snapshots.
  4. Mark closed.
- A DB trigger rejecting lines whose entry's period is closed.
- Reports: `trial_balance(as_of)` and `general_ledger(account, from, to)`.

**5. Testing Requirements**
- Posting into a closed period is rejected (engine and DB).
- Snapshot + lines after it = current balance.
- Close while postings are in flight: no posting lands in the closed period (concurrency test).
- The trial balance always balances.

**6. Deliverables**
Periods module and the two reports.

**7. Acceptance Criteria**
Snapshots verified against the full recomputation for 30 simulated days.

**8. Dependencies**
Week 13.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The close procedure is documented as a runbook.

---

## Week 22: REST API Design (Contract First)

**1. Overall Objective**
Design the public API from the now-stable model before implementing it.

**2. The Problem We Are Solving**
An API is a long-term promise. Mistakes become permanent for every client.

**3. Concepts and Research**
- Resource design for ledgers (accounts, entries, holds, operations).
- The `Idempotency-Key` header.
- RFC 9457 Problem Details.
- Cursor pagination.
- Amounts as strings.
- Versioning (`/v1`).
- OWASP API Security Top 10.
- OpenAPI 3.1.

**4. Implementation Tasks**
- Write `openapi/mezgeb.v1.yaml`. Endpoints:
  - accounts: create and get
  - balance at a point in time
  - entry history
  - posting operations (one per template)
  - reversal
  - holds
  - lookup by idempotency key
  - periods
  - reports
  - offline endpoints, stubbed
- An error code catalogue mapped to `PostingError`.
- A Postman collection generated from the spec.

**5. Testing Requirements**
- Spec lint.
- A review of every endpoint against OWASP API Top 10.
- Examples for each error code.

**6. Deliverables**
OpenAPI spec, error catalogue and Postman collection.

**7. Acceptance Criteria**
Every template and engine operation is reachable, and no endpoint allows raw arbitrary lines except an admin-only adjustment.

**8. Dependencies**
Weeks 17–21.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The spec is reviewed and frozen at v1-draft.

---

## Week 23: REST API Implementation (Axum)

**1. Overall Objective**
Implement the API exactly as specified.

**2. The Problem We Are Solving**
The HTTP layer is where timeouts, request limits and leaked errors creep in.

**3. Concepts and Research**
- Axum extractors and middleware (tower).
- Request body limits.
- Timeouts; what a client timeout does *not* cancel in the DB.
- Graceful shutdown that finishes in-flight transactions.
- Request IDs.

**4. Implementation Tasks**
- `mezgeb-api` crate: handlers call the templates and engine.
- Middleware: request ID, body limit, timeout, and a structured access log without sensitive fields.
- Problem Details error mapping.
- Contract tests generated from the OpenAPI spec.

**5. Testing Requirements**
- Postman collection runs in CI against a containerised service.
- A client timeout followed by a retry gives a single entry.
- Malformed amount, negative amount, float amount and huge payload are all rejected with the right code.
- Graceful shutdown during load loses no acknowledged request.

**6. Deliverables**
Running API service and CI contract tests.

**7. Acceptance Criteria**
100% of the spec's endpoints pass contract tests.

**8. Dependencies**
Week 22.

**9. Estimated Difficulty**
Medium.

**10. Exit Gate**
The chaos test from Week 15 re-runs through the API layer and comes out clean.

---

## Week 24: Authentication, Authorization and Privilege Separation

**1. Overall Objective**
Make sure only the right caller can perform each financial operation.

**2. The Problem We Are Solving**
The ledger's biggest risk is a legitimate-looking request from the wrong
party, such as a compromised client or a rogue admin.

**3. Concepts and Research**
- Service-to-service authentication: mTLS vs signed tokens vs hashed API keys (ADR).
- RBAC roles: `poster`, `reader`, `operator`, `approver`, `auditor`, `admin`.
- Least privilege.
- Separation of duties.
- Per-client scoping (a client can only touch its own accounts).
- OWASP ASVS V4 (access control).

**4. Implementation Tasks**
- `clients` and `client_credentials` tables, with credentials hashed (Argon2) or certificates pinned.
- A permission matrix as data: role × operation × template.
- An authorization middleware with **default deny**.
- Maker-checker for adjustments and manual period reopen.
- Per-client rate limits.

**5. Testing Requirements**
Security tests:
- Every endpoint × every role matches the permission matrix; anything not explicitly allowed is denied.
- Client A cannot read or post against client B's accounts.
- Replaying a revoked credential fails.
- An approver cannot approve their own request.

**6. Deliverables**
Authn/authz module, the permission matrix and the authorization test suite.

**7. Acceptance Criteria**
The auto-generated role × endpoint test matrix passes, and no endpoint lacks an authorization test.

**8. Dependencies**
Week 23.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The threat model's "compromised client" and "insider" threats are each mapped to a passing test.

---

## Week 25: Audit Logging and Tamper Evidence

**1. Overall Objective**
Record who did what and when, and make silent tampering detectable.

**2. The Problem We Are Solving**
A database superuser can rewrite history without leaving a trace. Auditors
need evidence that this didn't happen.

**3. Concepts and Research**
- Append-only audit events.
- Hash chains (each entry hash covers the previous hash).
- Signed checkpoints (Ed25519).
- External anchoring (publishing checkpoint hashes outside the DB).
- What tamper *evidence* can and cannot prove, as opposed to tamper *proof*.

**4. Implementation Tasks**
- `audit_events` table: actor, client, action, target, request ID, outcome, before/after for non-ledger config. Append-only.
- A hash chain on `journal_entries` (`prev_hash`, `hash`) computed in the posting transaction (the chain head row is locked; measure the contention cost).
- Daily signed checkpoints.
- `mezgeb-cli verify --chain`.

**5. Testing Requirements**
- Tampering with any entry, line or audit row (as superuser) is detected by the chain verify.
- Deleting the last entry is detected via the checkpoint.
- Benchmark posting throughput with and without the chain. If the chain head becomes a bottleneck, record it for Week 35.

**6. Deliverables**
Audit module, hash chain and checkpoint signer.

**7. Acceptance Criteria**
All planted tampering is detected, and every API mutation produces an audit event.

**8. Dependencies**
Weeks 13, 24.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
Where the checkpoint is published is decided (ADR).

---

## Week 26: Offline Architecture and Protocol Design (Design Only)

**1. Overall Objective**
Finalise the offline payment protocol and its accounting before writing code.

**2. The Problem We Are Solving**
Offline payments break the online assumptions: no central lock, untrusted
clocks, devices that may be cloned, and data that arrives late or never.
Getting the protocol wrong cannot be patched after cards ship.

**3. Concepts and Research**
- Escrow ("bounded counter") techniques.
- Secure elements and monotonic counters.
- Android Keystore/StrongBox key attestation.
- Offline data authentication in card schemes (as background, not to implement).
- Hash chains.
- Replay attacks.
- Risk limits per trust tier.
- Loss policy for late receipts.

**4. Implementation Tasks**
- Complete spec §5 with:
  - message formats (voucher, receipt) with canonical encoding
  - signature scheme
  - key hierarchy (issuer key, device keys)
  - device registration and attestation
  - trust tiers with limits and expiry
  - full state machines for voucher and receipt
  - the gap timeout
  - conflict handling
  - the dispute path
  - the loss policy
- An offline threat model addendum.
- Worked journal entries for every offline state.
- Align with the card hardware work: confirm the chip supports keys, a counter and signing.

**5. Testing Requirements**
- Desk-check every threat against the protocol.
- Walk 10 adversarial scenarios on paper: clone, replay, forged amount, clock rollback, payee double-upload, payer never syncs, lost device, partial sync, voucher expiry race, colluding merchant.

**6. Deliverables**
Offline spec v1, threat addendum and the scenario walkthroughs.

**7. Acceptance Criteria**
Every scenario has a defined outcome and a defined accounting result.

**8. Dependencies**
Weeks 14, 25; card hardware decisions.

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
External security review of the protocol requested (see README, "Specialist review").

---

## Week 27: Device Registry and Voucher Issuance

**1. Overall Objective**
Register devices and issue signed offline allowances backed by real escrow.

**2. The Problem We Are Solving**
Offline spending must be backed by money that is already reserved, or the
ledger takes on unbounded risk.

**3. Concepts and Research**
- Ed25519.
- Canonical encoding (fixed binary layout, no ambiguity).
- Key storage for the issuer key (start with an encrypted file / KMS; evaluate an HSM in Week 44).
- Per-device escrow accounts.

**4. Implementation Tasks**
- `devices` table (id, owner account, tier, public key, attestation result, status `active|frozen|revoked`).
- `vouchers` table.
- `issue_voucher.v1` template: a hold from the wallet into the device escrow, then a signed voucher.
- Tier limits enforced (configured per tier).
- `revoke_device`.
- `mezgeb-offline` canonical encoders.

**5. Testing Requirements**
- Issuing beyond the tier limit is rejected.
- Issuing for a frozen device is rejected.
- Voucher signatures verify, and tampering with a single bit fails.
- Concurrent issuance can't exceed the wallet balance.
- Known-answer test vectors for the encoding, committed so device firmware can test against them.

**6. Deliverables**
Device registry, voucher issuance and test vectors.

**7. Acceptance Criteria**
Escrow balance = SUM(unspent voucher value) at all times (an invariant).

**8. Dependencies**
Week 26.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
Test vectors are shared with the card/mobile firmware work.

---

## Week 28: Receipt Ingestion and Settlement

**1. Overall Objective**
Settle offline receipts exactly once, whatever the order, uploader or number
of repeats.

**2. The Problem We Are Solving**
The same receipt may arrive from the payer and the payee, days apart, out of
order. The money must move once.

**3. Concepts and Research**
- Idempotent ingestion keyed on `(device_id, counter)`.
- Validation order (cheap checks first, signature before any DB write that matters).
- Batch ingestion with a result per item.

**4. Implementation Tasks**
- `receipts` table with primary key `(device_id, counter)`.
- An ingestion pipeline:
  1. Decode.
  2. Verify the voucher.
  3. Verify the signature.
  4. Check the counter is within the voucher range.
  5. Check the chain link.
  6. Check the remaining escrow.
  7. Settle (escrow → payee) in the same transaction as inserting the receipt.
- Outcomes as in spec §5.5.
- The `POST /v1/offline/receipts` batch endpoint.

**5. Testing Requirements**
- The same receipt uploaded by payer and by payee: one settlement and two `DUPLICATE`/`SETTLED` results.
- Reversed upload order: same final state.
- A forged signature is rejected with no DB changes.
- Overspending the voucher → `OVER_LIMIT`.
- 100 concurrent uploads of one receipt → one settlement.

**6. Deliverables**
Ingestion pipeline and endpoint.

**7. Acceptance Criteria**
INV-9 and INV-10 hold in property tests with random upload orderings.

**8. Dependencies**
Week 27.

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
End-to-end demo: issue → offline receipts (simulated device) → sync → balances correct.

---

## Week 29: Gaps, Conflicts, Expiry, Rejections and Disputes

**1. Overall Objective**
Handle every non-happy offline outcome deterministically.

**2. The Problem We Are Solving**
The dangerous cases are the unusual ones: clones, missing receipts, late
receipts after escrow returned, and receipts users dispute.

**3. Concepts and Research**
- Gap timeout policy.
- Conflict evidence preservation.
- Device freeze cascade (stop issuing, flag the account).
- Escrow return on expiry.
- Suspense for late receipts.
- A dispute workflow with human review.

**4. Implementation Tasks**
- `WAITING_FOR_GAP` handling: settle receipts with a contiguous chain, hold later ones, and set the gap deadline.
- On a conflict:
  1. Freeze the device.
  2. Store both receipts as evidence.
  3. Settle nothing beyond the last contiguous verified counter.
  4. Raise an alert.
- An expiry worker returns unspent escrow to the wallet.
- A late receipt after the return is booked to a suspense account plus a dispute case (`offline_late_receipt.v1`).
- `disputes` table and the operator endpoints to resolve them.

**5. Testing Requirements**
- A clone (two receipts with the same counter) → frozen device; neither conflicting receipt settles automatically.
- Gap filled later → settles in order.
- A gap never filled → behaviour at the deadline matches the spec.
- Expiry racing a receipt ingestion: exactly one outcome, and the accounting is correct.
- Late receipt → suspense, and the case is opened.

**6. Deliverables**
Conflict, gap and expiry handling; the dispute workflow.

**7. Acceptance Criteria**
INV-11 holds, and every scenario walkthrough from Week 26 is an automated test.

**8. Dependencies**
Week 28.

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
All 10 adversarial scenarios pass as integration tests.

---

## Week 30: Offline Deterministic Simulation

**1. Overall Objective**
Prove the offline protocol at scale under adversarial conditions.

**2. The Problem We Are Solving**
Hand-written tests cover tens of cases. Real fleets produce millions of
interleavings.

**3. Concepts and Research**
- Deterministic simulation testing (FoundationDB, TigerBeetle VOPR).
- Seeded randomness.
- Simulated clocks.
- Fault schedules.
- Invariant checking every step.
- Reproducing a failure from its seed.

**4. Implementation Tasks**
- `mezgeb-sim` v2:
  - thousands of simulated devices with tiers
  - random payments
  - random sync timing and order
  - duplicate and lost receipts
  - cloned devices
  - clock skew
  - voucher expiry
  - ledger crashes
- Runs against the in-memory model and against PG (slower; nightly).
- Prints the seed and step on failure.
- Nightly CI job.

**5. Testing Requirements**
- 10M simulated operations per nightly run.
- Invariants INV-1 to INV-12 after every step.
- Planted bugs (skip the counter check, skip the chain check) are caught within one nightly run.

**6. Deliverables**
Simulator v2 and the nightly job.

**7. Acceptance Criteria**
14 consecutive clean nightly runs, and every planted bug caught.

**8. Dependencies**
Week 29.

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
A failing seed reproduces identically on another machine.

---

## Week 31: Reconciliation (Internal, External, Offline)

**1. Overall Objective**
Systematically detect any disagreement between the ledger and every other
record of money.

**2. The Problem We Are Solving**
The ledger can be internally perfect and still disagree with the bank
statement, a client's cached balances or device reports.

**3. Concepts and Research**
- Reconciliation types: internal (ledger vs caches), external (settlement account vs bank statement), offline (device-reported counters vs ledger).
- Matching rules.
- Breaks, ageing and resolution.
- The rule that the ledger wins over caches.

**4. Implementation Tasks**
- `recon_runs`, `recon_items` and `recon_breaks` tables.
- Statement import (CSV first; the format comes from the partner bank; ISO 20022 camt.053 only if the bank offers it).
- Matching engine (exact, then rule-based).
- Cache reconciliation endpoint for clients: per-wallet balance comparison.
- Resolution through suspense templates only.

**5. Testing Requirements**
- Planted mismatches (missing statement line, amount mismatch, duplicate statement line, stale cache) are each detected and classified.
- Resolution posts entries; it never edits.
- Reconciling 1M items completes and its runtime is recorded.

**6. Deliverables**
Reconciliation module.

**7. Acceptance Criteria**
100% of planted breaks are detected, with 0 false matches on the test dataset.

**8. Dependencies**
Weeks 18, 21, 29.

**9. Estimated Difficulty**
High.

**10. Exit Gate**
The daily reconciliation process is documented as a runbook.

---

## Week 32: Phase 2 Security Review and Exit

**1. Overall Objective**
Review the complete functional system against security standards before
optimising anything.

**2. The Problem We Are Solving**
Security flaws found after scaling and integration are much more expensive to
fix.

**3. Concepts and Research**
- OWASP ASVS Level 2 checklist.
- OWASP API Top 10.
- Business-logic abuse (negative amounts, rounding, races, replay).
- Dependency risk.

**4. Implementation Tasks**
- An ASVS L2 self-assessment written to `docs/security/asvs.md`.
- Abuse-case test suite.
- `cargo audit` and `cargo deny` in CI.
- Fix all high-severity findings.
- Update the threat model.
- Tag `v0.2.0-functional`.

**5. Testing Requirements**
- API fuzzing (property-based random requests against the running service): no 5xx caused by input, and no invariant violation.
- Authorization matrix re-run.
- Race abuse tests (refund, hold, voucher).

**6. Deliverables**
ASVS report, abuse suite and release tag.

**7. Acceptance Criteria**
No open high or critical findings, and every abuse case passes.

**8. Dependencies**
Weeks 17–31.

**9. Estimated Difficulty**
High.

**10. Exit Gate (Phase 2 Gate)**
- Every financial operation is booked through a template.
- The API is secured with default deny.
- Audit chain verified.
- Offline simulator clean for 14 nights.
- Reconciliation working.
