# Offline Protocol Test Vectors

Known-answer test vectors for [offline protocol v1](../../docs/offline/protocol.md),
for the teams building card firmware and phone apps. If your encoder and
verifier reproduce these, they agree with the ledger byte for byte.

| File | Covers |
|---|---|
| [v1.json](v1.json) | Test keys; a device certificate, a voucher and two chained receipts (fields, body, digest, signature); and five cases a verifier must refuse |

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

For each of the two `receipts`, signed by `device_key`:

1. **Encode** the `fields` and compare with `body`; **hash** and compare with
   `digest`.
2. **Verify** `signature` with `device_key.public_key`.
3. **Chain:** the first receipt's `prev_digest` is the voucher's `digest`;
   the second's is the first's `digest`.
4. **Sign** with `device_key.secret_key` (RFC 6979, low-S): the result must
   equal `signature`. This is what a card or phone does at each payment.

Then, for each case under `must_refuse`, your verifier must **refuse** the
signature over the body, with the issuer key or the key the case names:

- the voucher's signature in its high-S form (low-S only, protocol §2);
- the voucher with one bit of its body flipped;
- the device certificate's signature on the voucher's body;
- the first receipt's signature in its high-S form;
- the first receipt signed by the issuer key instead of the device key.

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
- **An optional payee device** is `0x00`, or `0x01` and its 16 bytes: the
  first receipt has one, the second does not.
- **Acknowledgements** are optional evidence and get their vectors when
  they are built.
