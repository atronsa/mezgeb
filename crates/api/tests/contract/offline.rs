//! Offline (steps 2.11, 2.12): register a device, issue a voucher and
//! check its signature with the published issuer key; upload receipts the
//! device signed, out of order and with repeats; the refusals; revoke.

use mezgeb_core::{Currency, Money, Timestamp};
use mezgeb_offline::{PublicKey, Receipt, SecretKey, Tier};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::common::{Api, BoxError, CLIENT, CLIENT_ID, OTHER, TestServer, etb};
use crate::{open, operate};

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

fn secret(label: &str) -> Result<SecretKey, BoxError> {
    Ok(SecretKey::from_bytes(
        &Sha256::digest(label.as_bytes()).into(),
    )?)
}

fn device_key(label: &str) -> Result<String, BoxError> {
    Ok(mezgeb_core::chain::hex(
        secret(label)?.public_key().compressed(),
    ))
}

fn hex(bytes: &[u8]) -> String {
    mezgeb_core::chain::hex(bytes)
}

pub async fn devices_and_vouchers(api: &Api, server: &TestServer) -> Result<(), BoxError> {
    let wallet = open(api, CLIENT, "2010:cust-offline").await?;
    expect!(
        operate(
            api,
            "deposit_confirmed.v1",
            "dep-offline",
            json!({
                "accounts": {"customer_funds": "1010", "customer_wallet": "2010:cust-offline"},
                "amounts": {"amount": etb("500.00")}})
        )
        .await?,
        201
    );
    let register = |key: String, attestation: &str| {
        json!({
            "owner_account": "2010:cust-offline",
            "public_key": key,
            "label": "Hana's phone",
            "attestation": {"type": attestation}
        })
    };
    let devices = "/v1/offline/devices";
    let phone = device_key("phone")?;

    // Tier C is off until the client enables it; cards are not verified yet.
    expect!(
        api.call(
            "POST",
            devices,
            Some(CLIENT),
            None,
            Some(register(phone.clone(), "none"))
        )
        .await?,
        422,
        "tier_disabled"
    );
    expect!(
        api.call(
            "POST",
            devices,
            Some(CLIENT),
            None,
            Some(register(phone.clone(), "card"))
        )
        .await?,
        422,
        "attestation_unsupported"
    );
    mezgeb_engine::set_tier_limits(
        &mut server.db.admin().await?,
        CLIENT_ID.parse()?,
        Tier::C,
        Some(Tier::C.maximum()),
    )
    .await?;
    let device = expect!(
        api.call(
            "POST",
            devices,
            Some(CLIENT),
            None,
            Some(register(phone.clone(), "none"))
        )
        .await?,
        201
    );
    assert_eq!(device.str("/tier"), "C");
    assert_eq!(device.str("/owner_account"), wallet);
    let device_id = device.str("/id");
    let again = expect!(
        api.call(
            "POST",
            devices,
            Some(CLIENT),
            None,
            Some(register(phone.clone(), "none"))
        )
        .await?,
        200
    );
    assert_eq!(again.str("/id"), device_id);
    let mut other = register(phone, "none");
    other["label"] = json!("someone else's");
    expect!(
        api.call("POST", devices, Some(CLIENT), None, Some(other))
            .await?,
        409,
        "public_key_registered"
    );
    let mut bank = register(device_key("bank")?, "none");
    bank["owner_account"] = json!("1010");
    expect!(
        api.call("POST", devices, Some(CLIENT), None, Some(bank))
            .await?,
        422,
        "owner_not_a_wallet"
    );
    expect!(
        api.get(&format!("{devices}/{device_id}"), CLIENT).await?,
        200
    );
    expect!(
        api.get(&format!("{devices}/{device_id}"), OTHER).await?,
        404
    );

    // A voucher, signed by a published issuer key.
    let vouchers = "/v1/offline/vouchers";
    let issue = |amount: &str| json!({"device": device_id, "amount": etb(amount)});
    expect!(
        api.post(vouchers, CLIENT, "vch-big", issue("200.01"))
            .await?,
        422,
        "over_tier_limit"
    );
    let voucher = expect!(
        api.post(vouchers, CLIENT, "vch-1", issue("100.00")).await?,
        201
    );
    let replay = expect!(
        api.post(vouchers, CLIENT, "vch-1", issue("100.00")).await?,
        200
    );
    assert_eq!(replay.str("/id"), voucher.str("/id"));
    expect!(
        api.post(vouchers, CLIENT, "vch-1", issue("99.00")).await?,
        409,
        "idempotency_key_reused"
    );
    expect!(
        api.post(vouchers, CLIENT, "vch-2", issue("10.00")).await?,
        409,
        "voucher_open"
    );
    let keys = expect!(api.get("/v1/offline/issuer-keys", CLIENT).await?, 200);
    let key = keys.body["items"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|k| k["key_id"] == voucher.body["issuer_key_id"])
        })
        .ok_or("the voucher's issuer key is not published")?;
    let issuer = PublicKey::from_compressed(&unhex(key["public_key"].as_str().unwrap_or("")))?;
    let body = unhex(&voucher.str("/body"));
    let signature = unhex(&voucher.str("/signature"));
    assert_eq!(issuer.verify(&body, &signature), Ok(()));
    let mut tampered = body.clone();
    tampered[40] ^= 1;
    assert!(issuer.verify(&tampered, &signature).is_err());
    expect!(
        api.get(&format!("{vouchers}/{}", voucher.str("/id")), CLIENT)
            .await?,
        200
    );
    expect!(
        api.get(&format!("{vouchers}/{}", voucher.str("/id")), OTHER)
            .await?,
        404
    );

    // Receipts the phone signed offline: 60 and 25 ETB to Merkato Coffee.
    let coffee = expect!(
        api.get("/v1/accounts?reference=2030:merch-coffee", CLIENT)
            .await?,
        200
    );
    let coffee = coffee.body["items"][0]["id"]
        .as_str()
        .ok_or("no coffee account")?
        .to_owned();
    let phone_key = secret("phone")?;
    let mut prev: [u8; 32] = Sha256::digest(&body).into();
    let mut signed = Vec::new();
    for (n, minor) in [(1u64, 6_000), (2, 2_500)] {
        let receipt = Receipt {
            device_id: device_id.parse()?,
            voucher_id: voucher.str("/id").parse()?,
            counter: n,
            prev_digest: prev,
            payee_account_id: coffee.parse()?,
            payee_device_id: None,
            amount: Money::from_minor(minor, Currency::ETB),
            event_time: Timestamp::from_micros(mezgeb_api::wire::now().as_micros() - 60_000_000),
            payee_challenge: [n as u8; 16],
        };
        prev = receipt.digest();
        signed.push(json!({
            "body": hex(&receipt.body()),
            "signature": hex(&receipt.sign(&phone_key)),
        }));
    }
    let upload = |items: Vec<serde_json::Value>| json!({ "receipts": items });
    let receipts = "/v1/offline/receipts";
    // The second arrives first: it waits for the first.
    let waiting = expect!(
        api.call(
            "POST",
            receipts,
            Some(CLIENT),
            None,
            Some(upload(vec![signed[1].clone()]))
        )
        .await?,
        200
    );
    assert_eq!(waiting.str("/results/0/outcome"), "WAITING_FOR_GAP");
    // The first settles both; the second again is a duplicate; a forged
    // signature and garbage are refused, each on its own.
    let mut forged = signed[0].clone();
    forged["signature"] = json!(hex(&[7; 64]));
    let garbage = json!({"body": "00", "signature": hex(&[7; 64])});
    let batch = expect!(
        api.call(
            "POST",
            receipts,
            Some(CLIENT),
            None,
            Some(upload(vec![
                signed[0].clone(),
                signed[1].clone(),
                forged,
                garbage
            ]))
        )
        .await?,
        200
    );
    let outcome = |i: usize| batch.str(&format!("/results/{i}/outcome"));
    assert_eq!(
        (outcome(0), outcome(1), outcome(2), outcome(3)),
        (
            "SETTLED".into(),
            "DUPLICATE".into(),
            "BAD_SIGNATURE".into(),
            "BAD_FORMAT".into()
        )
    );
    assert_eq!(
        batch.body["results"][0]["settled"].as_array().map(Vec::len),
        Some(2)
    );
    let listed = expect!(
        api.get(
            &format!("{vouchers}/{}/receipts", voucher.str("/id")),
            CLIENT
        )
        .await?,
        200
    );
    assert_eq!(listed.str("/items/1/outcome"), "SETTLED");
    expect!(
        api.get(
            &format!("{vouchers}/{}/receipts", voucher.str("/id")),
            OTHER
        )
        .await?,
        404
    );
    expect!(
        api.call("POST", receipts, Some(CLIENT), None, Some(upload(vec![])))
            .await?,
        400,
        "invalid_request"
    );

    // Revoked: no more vouchers.
    let revoked = expect!(
        api.post(
            &format!("{devices}/{device_id}/revoke"),
            CLIENT,
            "revoke-1",
            json!({"reason": "replaced"})
        )
        .await?,
        200
    );
    assert_eq!(revoked.str("/status"), "revoked");
    expect!(
        api.post(vouchers, CLIENT, "vch-3", issue("10.00")).await?,
        409,
        "device_not_active"
    );
    expect!(
        api.post(
            vouchers,
            CLIENT,
            "vch-4",
            json!({"device": "0192f3c4-0000-7000-8000-000000000bad", "amount": etb("1.00")})
        )
        .await?,
        404
    );
    Ok(())
}
