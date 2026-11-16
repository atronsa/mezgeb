//! Known-answer test vectors for offline protocol v1, committed in
//! `test-vectors/offline/v1.json` so card firmware and phone apps can test
//! their encoders and signers against the ledger's.
//!
//! The test rebuilds every vector and compares it with the committed file.
//! After a deliberate change to the protocol, regenerate the file with
//! `MEZGEB_WRITE_VECTORS=1 cargo test -p mezgeb-offline --test vectors`.

use std::path::PathBuf;

use mezgeb_core::chain::hex;
use mezgeb_core::{AccountId, ClientId, Currency, DeviceId, Money, Timestamp, VoucherId};
use mezgeb_offline::crypto::high_s;
use mezgeb_offline::{DeviceCertificate, PublicKey, SecretKey, Tier, Voucher};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-vectors/offline/v1.json")
}

/// A test key: the SHA-256 of a label, as a scalar.
fn test_key(label: &str) -> Result<SecretKey, Box<dyn std::error::Error>> {
    Ok(SecretKey::from_bytes(
        &Sha256::digest(label.as_bytes()).into(),
    )?)
}

fn key_json(label: &str, key: &SecretKey) -> Value {
    json!({
        "derivation": format!("SHA-256 of the ASCII text {label:?}, as a big-endian scalar"),
        "secret_key": hex(&Sha256::digest(label.as_bytes())),
        "public_key": hex(key.public_key().compressed()),
        "key_id": key.public_key().key_id(),
    })
}

fn flip_last_bit(body: &[u8]) -> Vec<u8> {
    let mut out = body.to_vec();
    if let Some(last) = out.last_mut() {
        *last ^= 1;
    }
    out
}

fn vectors() -> Result<Value, Box<dyn std::error::Error>> {
    let issuer = test_key("mezgeb offline test issuer v1")?;
    let device = test_key("mezgeb offline test device v1")?;
    let issuer_key_id = issuer.public_key().key_id();

    let device_id: DeviceId = "019a8f00-0000-7000-8000-000000000001".parse()?;
    let client_id: ClientId = "019a8f00-0000-7000-8000-000000000002".parse()?;
    let owner: AccountId = "019a8f00-0000-7000-8000-000000000003".parse()?;
    let voucher_id: VoucherId = "019a8f00-0000-7000-8000-000000000004".parse()?;
    let escrow: AccountId = "019a8f00-0000-7000-8000-000000000005".parse()?;
    // 2026-11-16T00:00:00Z.
    let issued_at = Timestamp::from_micros(1_794_787_200_000_000);

    let certificate = DeviceCertificate {
        device_id,
        client_id,
        owner_account_id: owner,
        public_key: PublicKey::from_compressed(device.public_key().compressed())?,
        tier: Tier::B,
        issued_at,
        valid_until: Timestamp::from_micros(
            issued_at.as_micros() + mezgeb_offline::CERTIFICATE_MICROS,
        ),
        issuer_key_id: issuer_key_id.clone(),
    };
    let voucher = Voucher {
        voucher_id,
        device_id,
        client_id,
        escrow_account_id: escrow,
        amount: Money::from_minor(100_000, Currency::ETB),
        counter_start: 1,
        issued_at,
        expires_at: Timestamp::from_micros(
            issued_at.as_micros() + Tier::B.maximum().lifetime_micros,
        ),
        issuer_key_id: issuer_key_id.clone(),
    };

    let cert_sig = certificate.sign(&issuer);
    let voucher_sig = voucher.sign(&issuer);
    let voucher_high_s = high_s(&voucher_sig).ok_or("no high-S form")?;

    Ok(json!({
        "protocol": "mezgeb offline protocol v1 (docs/offline/protocol.md)",
        "notes": [
            "Test keys only: never use them for real devices or issuers.",
            "Signatures are ECDSA P-256 with SHA-256 over the body, r || s, low-S, with RFC 6979 nonces, so they are reproducible.",
            "A verifier must accept every 'signature' and refuse every vector under 'must_refuse'.",
        ],
        "issuer_key": key_json("mezgeb offline test issuer v1", &issuer),
        "device_key": key_json("mezgeb offline test device v1", &device),
        "device_certificate": {
            "fields": {
                "device_id": device_id.to_string(),
                "client_id": client_id.to_string(),
                "owner_account_id": owner.to_string(),
                "public_key": hex(certificate.public_key.compressed()),
                "key_id": certificate.public_key.key_id(),
                "tier": certificate.tier.code(),
                "issued_at": certificate.issued_at.as_micros(),
                "valid_until": certificate.valid_until.as_micros(),
                "issuer_key_id": issuer_key_id,
            },
            "body": hex(&certificate.body()),
            "digest": hex(&certificate.digest()),
            "signature": hex(&cert_sig),
        },
        "voucher": {
            "fields": {
                "voucher_id": voucher_id.to_string(),
                "device_id": device_id.to_string(),
                "client_id": client_id.to_string(),
                "escrow_account_id": escrow.to_string(),
                "amount": { "currency": "ETB", "minor": voucher.amount.amount() },
                "counter_start": voucher.counter_start,
                "issued_at": voucher.issued_at.as_micros(),
                "expires_at": voucher.expires_at.as_micros(),
                "issuer_key_id": voucher.issuer_key_id,
            },
            "body": hex(&voucher.body()),
            "digest": hex(&voucher.digest()),
            "signature": hex(&voucher_sig),
        },
        "must_refuse": [
            {
                "what": "the voucher's signature in its high-S form",
                "body": hex(&voucher.body()),
                "signature": hex(&voucher_high_s),
            },
            {
                "what": "the voucher with the last bit of its body flipped",
                "body": hex(&flip_last_bit(&voucher.body())),
                "signature": hex(&voucher_sig),
            },
            {
                "what": "the device certificate's signature on the voucher's body",
                "body": hex(&voucher.body()),
                "signature": hex(&cert_sig),
            },
        ],
    }))
}

#[test]
fn the_committed_vectors_match() -> TestResult {
    let mut expected = serde_json::to_string_pretty(&vectors()?)?;
    expected.push('\n');
    if std::env::var_os("MEZGEB_WRITE_VECTORS").is_some() {
        std::fs::write(file(), &expected)?;
    }
    let committed = std::fs::read_to_string(file())?.replace("\r\n", "\n");
    assert_eq!(committed, expected, "test-vectors/offline/v1.json is stale");
    Ok(())
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[test]
fn the_vectors_verify_and_the_refusals_are_refused() -> TestResult {
    let v = vectors()?;
    let issuer = PublicKey::from_compressed(&unhex(
        v["issuer_key"]["public_key"].as_str().ok_or("no key")?,
    ))?;
    for name in ["device_certificate", "voucher"] {
        let body = unhex(v[name]["body"].as_str().ok_or("no body")?);
        let signature = unhex(v[name]["signature"].as_str().ok_or("no signature")?);
        assert_eq!(issuer.verify(&body, &signature), Ok(()), "{name}");
    }
    for case in v["must_refuse"].as_array().ok_or("no refusals")? {
        let body = unhex(case["body"].as_str().ok_or("no body")?);
        let signature = unhex(case["signature"].as_str().ok_or("no signature")?);
        assert!(
            issuer.verify(&body, &signature).is_err(),
            "{}",
            case["what"]
        );
    }
    Ok(())
}
