# Offline Protocol Test Vectors

Known-answer test vectors for [offline protocol v1](../../docs/offline/protocol.md),
for the teams building card firmware and phone apps. If your encoder and
verifier reproduce these, they agree with the ledger byte for byte.

| File | Covers |
|---|---|
| [v1.json](v1.json) | Test keys, a device certificate and a voucher (fields, body, digest, signature), and three cases a verifier must refuse |

## How to use them

For each of `device_certificate` and `voucher`:

1. **Encode** the `fields` with the canonical encoding (protocol §3) and
   compare with `body` (hex). Every byte must match.
2. **Hash** the body with SHA-256 and compare with `digest`.
3. **Verify** `signature` over the body with `issuer_key.public_key`:
   ECDSA P-256, SHA-256, 64 bytes `r ‖ s`. It must verify.
4. **Sign** the body with `issuer_key.secret_key` using RFC 6979
   deterministic nonces, then make `s` low (`s ≤ n/2`). The result must
   equal `signature`. A signer without RFC 6979 can skip this check; its
   signatures must still verify and be low-S.

Then, for each case under `must_refuse`, your verifier must **refuse** the
signature over the body:

- the voucher's signature in its high-S form (low-S only, protocol §2);
- the voucher with one bit of its body flipped;
- the device certificate's signature on the voucher's body.

## Notes

- **The keys are test keys,** derived from fixed text, and published here.
  Never put them in a real device or a real ledger.
- **Times** are microseconds since the Unix epoch, as `i64`.
- **IDs** are UUIDv7, encoded as their 16 bytes.
- **Amounts** are the currency code as text, then the minor units as `i64`:
  `1000.00 ETB` is `ETB` and `100000`.
- **The file is checked by the ledger's tests** (`crates/offline/tests/vectors.rs`):
  it changes only with the protocol, and the change shows in review. It was
  also checked with an independent implementation (Python `cryptography`).
- **Receipts and acknowledgements** get their vectors with step 2.12.
