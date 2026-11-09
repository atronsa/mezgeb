# 2.12 Receipt Ingestion and Settlement

[Phase 2: Financial Operations, API, Security and Offline](README.md) · Previous: [2.11 Device Registry and Voucher Issuance](11-device-registry-and-vouchers.md) · Next: [2.13 Gaps, Conflicts, Expiry, Rejections and Disputes](13-offline-conflicts-and-expiry.md)

---

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
Step [2.11](../phase-2-operations-and-security/11-device-registry-and-vouchers.md).

**9. Estimated Difficulty**
Very High.

**10. Exit Gate**
End-to-end demo: issue → offline receipts (simulated device) → sync → balances correct.
