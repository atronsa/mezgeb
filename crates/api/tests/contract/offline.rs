//! Offline (step 2.11): register a device, issue a voucher and check its
//! signature with the published issuer key, the refusals, revoke; receipts
//! still answer `501`.

use mezgeb_offline::{PublicKey, SecretKey, Tier};
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

fn device_key(label: &str) -> Result<String, BoxError> {
    let key = SecretKey::from_bytes(&Sha256::digest(label.as_bytes()).into())?;
    Ok(mezgeb_core::chain::hex(key.public_key().compressed()))
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

pub async fn planned(api: &Api) -> Result<(), BoxError> {
    expect!(
        api.call(
            "POST",
            "/v1/offline/receipts",
            Some(CLIENT),
            None,
            Some(json!({}))
        )
        .await?,
        501,
        "not_implemented"
    );
    Ok(())
}
