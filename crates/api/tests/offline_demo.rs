//! The exit gate of step 2.12, end to end through the API: a voucher is
//! issued, a simulated device signs receipts offline, the payee's terminal
//! and then the payer's phone sync them (out of order, with repeats), and
//! every balance comes out right.
//!
//! Run with `--nocapture` to see the story as it happens.

mod common;

use std::time::Duration;

use common::{Api, BoxError, CLIENT, CLIENT_ID, Contract, OPERATOR, TestServer, etb};
use mezgeb_core::{Currency, Money, Timestamp};
use mezgeb_offline::{Receipt, SecretKey, Tier};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    mezgeb_core::chain::hex(bytes)
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

async fn ok(api: &Api, reply: common::Reply) -> Result<Value, BoxError> {
    let _ = api;
    if !(200..300).contains(&reply.status) {
        return Err(format!("{}: {}", reply.status, reply.body).into());
    }
    Ok(reply.body)
}

async fn open(api: &Api, token: &str, reference: &str) -> Result<String, BoxError> {
    let reply = api
        .post(
            "/v1/accounts",
            token,
            &format!("open-{reference}"),
            json!({"reference": reference}),
        )
        .await?;
    Ok(ok(api, reply).await?["id"]
        .as_str()
        .ok_or("no id")?
        .to_owned())
}

async fn balance(api: &Api, account: &str) -> Result<String, BoxError> {
    let reply = api.get(&format!("/v1/accounts/{account}"), CLIENT).await?;
    let body = ok(api, reply).await?;
    Ok(body["balance"]["amount"]
        .as_str()
        .ok_or("no balance")?
        .to_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_spend_offline_sync_and_the_balances_are_right() -> Result<(), BoxError> {
    let Some(server) = TestServer::start(Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let api = Api::new(
        &server.base,
        std::sync::Arc::new(Contract::load().map_err(|e| e.to_string())?),
    );

    // The platform: its accounts, a 2% offline merchant fee, tier C on.
    for reference in ["1010", "4010"] {
        open(&api, OPERATOR, reference).await?;
    }
    let hana = open(&api, CLIENT, "2010:cust-hana").await?;
    let coffee = open(&api, CLIENT, "2030:merch-coffee").await?;
    let stall = open(&api, CLIENT, "2030:merch-stall").await?;
    let mut admin = server.db.admin().await?;
    mezgeb_engine::set_tier_limits(
        &mut admin,
        CLIENT_ID.parse()?,
        Tier::C,
        Some(Tier::C.maximum()),
    )
    .await?;
    mezgeb_engine::set_offline_fee(&mut admin, CLIENT_ID.parse()?, 200).await?;
    drop(admin);
    ok(
        &api,
        api.post(
            "/v1/operations/deposit_confirmed.v1",
            CLIENT,
            "dep-hana",
            json!({
                "accounts": {"customer_funds": "1010", "customer_wallet": "2010:cust-hana"},
                "amounts": {"amount": etb("500.00")}
            }),
        )
        .await?,
    )
    .await?;
    println!(
        "Hana deposits 500.00 ETB; her wallet holds {}",
        balance(&api, &hana).await?
    );

    // Hana's phone registers and takes a 200 ETB voucher.
    let phone = SecretKey::from_bytes(&Sha256::digest(b"hana's phone").into())?;
    let device = ok(
        &api,
        api.call(
            "POST",
            "/v1/offline/devices",
            Some(CLIENT),
            None,
            Some(json!({
                "owner_account": "2010:cust-hana",
                "public_key": hex(phone.public_key().compressed()),
                "label": "Hana's phone",
                "attestation": {"type": "none"}
            })),
        )
        .await?,
    )
    .await?;
    let device_id = device["id"].as_str().ok_or("no device")?.to_owned();
    let voucher = ok(
        &api,
        api.post(
            "/v1/offline/vouchers",
            CLIENT,
            "vch-hana-1",
            json!({"device": device_id, "amount": etb("200.00")}),
        )
        .await?,
    )
    .await?;
    let escrow = voucher["escrow_account"]
        .as_str()
        .ok_or("no escrow")?
        .to_owned();
    println!(
        "The phone gets a 200.00 ETB voucher: wallet {}, escrow {}",
        balance(&api, &hana).await?,
        balance(&api, &escrow).await?
    );

    // Offline, at the market: three payments, signed on the phone.
    let mut prev: [u8; 32] =
        Sha256::digest(unhex(voucher["body"].as_str().ok_or("no body")?)).into();
    let mut signed = Vec::new();
    let now = mezgeb_api::wire::now().as_micros();
    for (n, (payee, minor, what)) in [
        (&coffee, 6_000, "coffee, 60.00"),
        (&stall, 4_550, "vegetables, 45.50"),
        (&coffee, 1_200, "tea, 12.00"),
    ]
    .into_iter()
    .enumerate()
    {
        let receipt = Receipt {
            device_id: device_id.parse()?,
            voucher_id: voucher["id"].as_str().ok_or("no voucher")?.parse()?,
            counter: n as u64 + 1,
            prev_digest: prev,
            payee_account_id: payee.parse()?,
            payee_device_id: None,
            amount: Money::from_minor(minor, Currency::ETB),
            event_time: Timestamp::from_micros(now - (3 - n as i64) * 60_000_000),
            payee_challenge: [n as u8 + 1; 16],
        };
        prev = receipt.digest();
        println!("  offline receipt {}: {what} ETB", receipt.counter);
        signed.push(json!({
            "body": hex(&receipt.body()),
            "signature": hex(&receipt.sign(&phone)),
        }));
    }

    // The stall's terminal syncs first: its receipt waits for the first.
    let upload = |items: &[Value]| json!({ "receipts": items });
    let stall_sync = ok(
        &api,
        api.call(
            "POST",
            "/v1/offline/receipts",
            Some(CLIENT),
            None,
            Some(upload(&signed[1..2])),
        )
        .await?,
    )
    .await?;
    println!(
        "The stall syncs receipt 2: {}",
        stall_sync["results"][0]["outcome"]
    );
    // The coffee shop syncs its two: 1 settles, then 2 that was waiting, then 3.
    let coffee_sync = ok(
        &api,
        api.call(
            "POST",
            "/v1/offline/receipts",
            Some(CLIENT),
            None,
            Some(upload(&[signed[0].clone(), signed[2].clone()])),
        )
        .await?,
    )
    .await?;
    println!(
        "The coffee shop syncs receipts 1 and 3: {} (settling {} receipts), {}",
        coffee_sync["results"][0]["outcome"],
        coffee_sync["results"][0]["settled"]
            .as_array()
            .map_or(0, Vec::len),
        coffee_sync["results"][1]["outcome"]
    );
    // Back online, Hana's phone uploads everything it signed: all duplicates.
    let phone_sync = ok(
        &api,
        api.call(
            "POST",
            "/v1/offline/receipts",
            Some(CLIENT),
            None,
            Some(upload(&signed)),
        )
        .await?,
    )
    .await?;
    let outcomes: Vec<&str> = phone_sync["results"]
        .as_array()
        .ok_or("no results")?
        .iter()
        .filter_map(|r| r["outcome"].as_str())
        .collect();
    println!("Hana's phone syncs all three: {outcomes:?}");
    assert_eq!(outcomes, ["DUPLICATE"; 3]);

    // The balances: 117.50 spent from escrow, each merchant paid less 2%.
    let wallet = balance(&api, &hana).await?;
    let left = balance(&api, &escrow).await?;
    let coffee_balance = balance(&api, &coffee).await?;
    let stall_balance = balance(&api, &stall).await?;
    println!(
        "Balances: wallet {wallet}, escrow {left}, Merkato Coffee {coffee_balance}, stall {stall_balance}"
    );
    assert_eq!(wallet, "300.00");
    assert_eq!(left, "82.50");
    assert_eq!(coffee_balance, "70.56"); // 72.00 less 1.20 and 0.24
    assert_eq!(stall_balance, "44.59"); // 45.50 less 0.91
    let fees = api.get("/v1/accounts?reference=4010", OPERATOR).await?;
    let fees_id = ok(&api, fees).await?["items"][0]["id"]
        .as_str()
        .ok_or("no fee account")?
        .to_owned();
    let fee_balance = ok(
        &api,
        api.get(&format!("/v1/accounts/{fees_id}"), OPERATOR)
            .await?,
    )
    .await?["balance"]["amount"]
        .as_str()
        .ok_or("no balance")?
        .to_owned();
    println!("Fee revenue: {fee_balance}");
    assert_eq!(fee_balance, "2.35");

    // And the ledger agrees: every escrow holds its voucher's unspent value.
    let mut verifier = server.db.verifier().await?;
    assert_eq!(mezgeb_engine::escrow_findings(&mut verifier).await?, vec![]);
    println!("Escrow check: every escrow holds its unspent voucher value");
    drop(verifier);
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}
