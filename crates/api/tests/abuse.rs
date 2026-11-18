//! Business-logic abuse (step 2.16): what a client platform with valid
//! credentials, or a person with a device, tries in order to take money it
//! should not, through the API, as an attacker would.
//!
//! | Case | Abuse | Expected |
//! |---|---|---|
//! | AC-01 | Amounts that are not plain positive decimals: negative, zero, signs, exponents, other digits, too many places, numbers, overflow | `400` or `422`, never `2xx` or `5xx`; no balance moves |
//! | AC-02 | Balances pushed to the edge of `i64` | Refused cleanly, never wrapped, never `5xx` |
//! | AC-03 | Replays: the same key with the same body, the same key with another body, another client's key | One effect; `409 idempotency_key_reused`; keys are per client |
//! | AC-04 | Many partial refunds at once, more than the payment, and a fee refund larger than the fee | The payer gets back exactly what it paid, never a santim more |
//! | AC-05 | Overspending one wallet with many transfers at once | Exactly as many succeed as the balance covers; it never goes below zero |
//! | AC-06 | Capturing and releasing one hold many times at once | Exactly one wins; the money is either captured or back, never both |
//! | AC-07 | Several vouchers at once for one device, and the same receipts uploaded by many at once | One voucher; each receipt pays once |
//! | AC-08 | Rounding: splitting an offline payment to shave the merchant fee | At most one santim saved per split; the fee is never negative or more than the payment (`splitting_a_payment_saves_at_most_one_santim_of_fee`, `crates/engine/src/offline_receipts.rs`, the only fee the service computes) |
//! | AC-09 | Naming another client's entry as the one to refund, to learn whether it exists | Answered exactly as an entry that does not exist: `422 refunded_entry_not_found` (found by the fuzzer, `tests/fuzz.rs`) |
//!
//! Every test ends with the ledger's full verification (`TestDb::drop`):
//! balances, totals, chains and escrows.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Api, BoxError, CLIENT, CLIENT_ID, Contract, OPERATOR, OTHER, TestServer, etb};
use mezgeb_core::{Currency, Money, Timestamp};
use mezgeb_offline::{Receipt, SecretKey, Tier};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

async fn server() -> Result<Option<(TestServer, Api)>, BoxError> {
    let Some(server) = TestServer::start(Duration::from_secs(60))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    let api = Api::new(&server.base, contract);
    Ok(Some((server, api)))
}

/// Runs the futures concurrently and collects their results in order.
async fn all<F, T>(futures: impl Iterator<Item = F>) -> Result<Vec<T>, BoxError>
where
    F: std::future::Future<Output = Result<T, BoxError>> + Send + 'static,
    T: Send + 'static,
{
    let tasks: Vec<_> = futures.map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(tasks.len());
    for task in tasks {
        out.push(task.await??);
    }
    Ok(out)
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
    assert!(reply.status == 201 || reply.status == 200, "{}", reply.body);
    Ok(reply.str("/id"))
}

async fn balance(api: &Api, id: &str) -> Result<String, BoxError> {
    let reply = api.get(&format!("/v1/accounts/{id}"), OPERATOR).await?;
    assert_eq!(reply.status, 200, "{}", reply.body);
    Ok(reply.str("/balance/amount"))
}

async fn operate(
    api: &Api,
    template: &str,
    key: &str,
    body: Value,
) -> Result<common::Reply, BoxError> {
    api.post(&format!("/v1/operations/{template}"), CLIENT, key, body)
        .await
}

async fn deposit(
    api: &Api,
    wallet: &str,
    amount: &str,
    key: &str,
) -> Result<common::Reply, BoxError> {
    operate(
        api,
        "deposit_confirmed.v1",
        key,
        json!({"accounts": {"customer_funds": "1010", "customer_wallet": wallet},
               "amounts": {"amount": etb(amount)}}),
    )
    .await
}

fn transfer(from: &str, to: &str, amount: Value) -> Value {
    json!({"accounts": {"payer_wallet": from, "payee_wallet": to},
           "amounts": {"amount": amount}})
}

/// AC-01.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac01_amounts_that_are_not_plain_positive_decimals_are_refused() -> Result<(), BoxError> {
    let Some((server, api)) = server().await? else {
        return Ok(());
    };
    open(&api, OPERATOR, "1010").await?;
    let a = open(&api, CLIENT, "2010:cust-a").await?;
    let b = open(&api, CLIENT, "2010:cust-b").await?;
    let funded = deposit(&api, "2010:cust-a", "100.00", "fund-a").await?;
    assert_eq!(funded.status, 201, "{}", funded.body);

    let texts = [
        "-1.00",
        "0.00",
        "-0.00",
        "+1.00",
        "1e2",
        "1E2",
        "1.001",
        "0.001",
        ".50",
        "1.",
        " 1.00",
        "1.00 ",
        "１.００",
        "١.٠٠",
        "1,00",
        "1_000.00",
        "0x10",
        "NaN",
        "Infinity",
        "",
        "92233720368547758.08",
        "99999999999999999999999999.99",
        "1.00\u{0000}",
    ];
    let mut amounts: Vec<Value> = texts
        .iter()
        .map(|t| json!({"amount": t, "currency": "ETB"}))
        .collect();
    amounts.extend([
        json!({"amount": 1.0, "currency": "ETB"}),
        json!({"amount": 1, "currency": "ETB"}),
        json!({"amount": null, "currency": "ETB"}),
        json!({"amount": ["1.00"], "currency": "ETB"}),
        json!({"amount": "1.00"}),
        json!({"amount": "1.00", "currency": "etb"}),
        json!({"amount": "1.00", "currency": "ETB "}),
        json!({"amount": "1.00", "currency": "USD"}),
        json!("1.00"),
        json!(null),
    ]);
    for (n, amount) in amounts.into_iter().enumerate() {
        let reply = operate(
            &api,
            "p2p_transfer.v1",
            &format!("bad-{n}"),
            transfer("2010:cust-a", "2010:cust-b", amount.clone()),
        )
        .await?;
        assert!(
            reply.status == 400 || reply.status == 422,
            "{amount}: {} {}",
            reply.status,
            reply.body
        );
    }
    assert_eq!(balance(&api, &a).await?, "100.00");
    assert_eq!(balance(&api, &b).await?, "0.00");
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// AC-02.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac02_balances_at_the_edge_of_i64_are_refused_cleanly() -> Result<(), BoxError> {
    let Some((server, api)) = server().await? else {
        return Ok(());
    };
    open(&api, OPERATOR, "1010").await?;
    let a = open(&api, CLIENT, "2010:cust-a").await?;
    // The largest amount there is: 2^63 - 1 santim.
    let max = "92233720368547758.07";
    let first = deposit(&api, "2010:cust-a", max, "max-1").await?;
    assert_eq!(first.status, 201, "{}", first.body);
    // Once more, or a santim more: the balance would pass i64::MAX.
    for (key, amount) in [("max-2", max), ("max-3", "0.01")] {
        let reply = deposit(&api, "2010:cust-a", amount, key).await?;
        assert!(
            (400..500).contains(&reply.status),
            "{amount} on top of the maximum: {} {}",
            reply.status,
            reply.body
        );
    }
    assert_eq!(balance(&api, &a).await?, max, "never wrapped");
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// AC-03.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac03_replays_have_one_effect_and_keys_are_per_client() -> Result<(), BoxError> {
    let Some((server, api)) = server().await? else {
        return Ok(());
    };
    open(&api, OPERATOR, "1010").await?;
    let a = open(&api, CLIENT, "2010:cust-a").await?;
    let b = open(&api, CLIENT, "2010:cust-b").await?;
    open(&api, OTHER, "2010:cust-oa").await?;
    open(&api, OTHER, "2010:cust-ob").await?;
    deposit(&api, "2010:cust-a", "100.00", "fund-a").await?;

    // The same request, replayed ten times at once: one entry.
    let body = transfer("2010:cust-a", "2010:cust-b", etb("10.00"));
    let replies = all((0..10).map(|_| {
        let (api, body) = (api.clone(), body.clone());
        async move { operate(&api, "p2p_transfer.v1", "pay-once", body).await }
    }))
    .await?;
    let ids: std::collections::BTreeSet<String> = replies
        .iter()
        .map(|r| {
            assert!(r.status == 201 || r.status == 200, "{}", r.body);
            r.str("/id")
        })
        .collect();
    assert_eq!(ids.len(), 1, "one entry for one key");
    assert_eq!(replies.iter().filter(|r| r.status == 201).count(), 1);
    assert!(
        replies
            .iter()
            .filter(|r| r.status == 200)
            .all(common::Reply::replayed)
    );
    assert_eq!(balance(&api, &a).await?, "90.00");
    assert_eq!(balance(&api, &b).await?, "10.00");

    // The same key with another amount, or other accounts: refused.
    for other in [
        transfer("2010:cust-a", "2010:cust-b", etb("99.00")),
        transfer("2010:cust-b", "2010:cust-a", etb("10.00")),
    ] {
        let reply = operate(&api, "p2p_transfer.v1", "pay-once", other).await?;
        assert_eq!(
            (reply.status, reply.code()),
            (409, "idempotency_key_reused"),
            "{}",
            reply.body
        );
    }
    // The same key under another template is another request too.
    let reply = operate(
        &api,
        "deposit_confirmed.v1",
        "pay-once",
        json!({"accounts": {"customer_funds": "1010", "customer_wallet": "2010:cust-a"},
               "amounts": {"amount": etb("10.00")}}),
    )
    .await?;
    assert_eq!(reply.code(), "idempotency_key_reused", "{}", reply.body);

    // Another client's key is its own: it neither replays nor reveals the
    // first client's entry.
    let entry = ids.into_iter().next().ok_or("no entry")?;
    let found = api
        .get("/v1/entries/by-idempotency-key/pay-once", OTHER)
        .await?;
    assert_eq!(found.code(), "not_found", "{}", found.body);
    let theirs = api
        .post(
            "/v1/operations/deposit_confirmed.v1",
            OTHER,
            "pay-once",
            json!({"accounts": {"customer_funds": "1010", "customer_wallet": "2010:cust-oa"},
                   "amounts": {"amount": etb("5.00")}}),
        )
        .await?;
    assert_eq!(theirs.status, 201, "{}", theirs.body);
    assert_ne!(theirs.str("/id"), entry);
    assert_eq!(balance(&api, &a).await?, "90.00");
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// AC-04.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac04_refunds_never_return_more_than_was_paid() -> Result<(), BoxError> {
    let Some((server, api)) = server().await? else {
        return Ok(());
    };
    open(&api, OPERATOR, "1010").await?;
    let fees = open(&api, OPERATOR, "4010").await?;
    let payer = open(&api, CLIENT, "2010:cust-a").await?;
    let merchant = open(&api, CLIENT, "2030:merch-coffee").await?;
    deposit(&api, "2010:cust-a", "500.00", "fund-a").await?;
    // The merchant has money of its own, so only the cap can stop a refund.
    deposit(&api, "2010:cust-a", "0.01", "fund-a2").await?;
    let other_sale = operate(&api, "merchant_payment.v1", "pay-0", json!({
        "accounts": {"payer_wallet": "2010:cust-a", "merchant_balance": "2030:merch-coffee", "fee_revenue": "4010"},
        "amounts": {"amount": etb("300.00"), "merchant_fee": etb("6.00")}, "metadata": {"channel": "qr"}})).await?;
    assert_eq!(other_sale.status, 201, "{}", other_sale.body);
    let payment = operate(&api, "merchant_payment.v1", "pay-1", json!({
        "accounts": {"payer_wallet": "2010:cust-a", "merchant_balance": "2030:merch-coffee", "fee_revenue": "4010"},
        "amounts": {"amount": etb("100.00"), "merchant_fee": etb("2.00")}, "metadata": {"channel": "qr"}})).await?;
    assert_eq!(payment.status, 201, "{}", payment.body);
    let paid = payment.str("/id");
    let refund = |amount: &str, fee: Option<&str>| {
        let mut amounts = json!({"amount": etb(amount)});
        if let Some(fee) = fee {
            amounts["fee_refund"] = etb(fee);
        }
        json!({"accounts": {"merchant_balance": "2030:merch-coffee", "fee_revenue": "4010",
                            "payer_wallet": "2010:cust-a"},
               "amounts": amounts, "refunds": paid})
    };

    // A fee refund larger than the fee the payment paid, or than the refund.
    for (n, (amount, fee)) in [("10.00", "2.01"), ("1.00", "1.01")].iter().enumerate() {
        let reply = operate(
            &api,
            "merchant_refund.v1",
            &format!("fee-{n}"),
            refund(amount, Some(fee)),
        )
        .await?;
        assert_eq!(reply.status, 422, "{}", reply.body);
    }

    // Forty refunds of 5.00 at once, all from the merchant's balance. The
    // cap is per account (INV-15): the merchant received 98.00 of the
    // 100.00, so at most 95.00 can come back from it in steps of 5.00.
    let before = balance(&api, &payer).await?;
    let replies = all((0..40).map(|n| {
        let (api, body) = (api.clone(), refund("5.00", None));
        async move { operate(&api, "merchant_refund.v1", &format!("ref-{n}"), body).await }
    }))
    .await?;
    let refunded = replies.iter().filter(|r| r.status == 201).count();
    assert_eq!(refunded, 19, "95.00 of the merchant's 98.00");
    for reply in replies.iter().filter(|r| r.status != 201) {
        assert_eq!(reply.code(), "refund_exceeds_original", "{}", reply.body);
    }
    // The last 5.00 needs the 2.00 of fee: 3.00 more from the merchant
    // makes its 98.00, and the fee its 2.00.
    let last = operate(
        &api,
        "merchant_refund.v1",
        "ref-last",
        refund("5.00", Some("2.00")),
    )
    .await?;
    assert_eq!(last.status, 201, "{}", last.body);
    let after = balance(&api, &payer).await?;
    let back = after.parse::<f64>()? - before.parse::<f64>()?;
    assert!((back - 100.00).abs() < 0.001, "{before} -> {after}");
    // Not one santim more, from either account.
    for (key, body) in [
        ("ref-more", refund("0.01", None)),
        ("ref-fee", refund("0.01", Some("0.01"))),
    ] {
        let reply = operate(&api, "merchant_refund.v1", key, body).await?;
        assert_eq!(reply.code(), "refund_exceeds_original", "{}", reply.body);
    }
    assert_eq!(
        balance(&api, &fees).await?,
        "6.00",
        "8.00 of fees less the 2.00 refunded"
    );
    assert!(!balance(&api, &merchant).await?.starts_with('-'));
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// AC-05.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac05_a_wallet_cannot_be_overspent_by_concurrent_transfers() -> Result<(), BoxError> {
    let Some((server, api)) = server().await? else {
        return Ok(());
    };
    open(&api, OPERATOR, "1010").await?;
    let a = open(&api, CLIENT, "2010:cust-a").await?;
    let b = open(&api, CLIENT, "2010:cust-b").await?;
    deposit(&api, "2010:cust-a", "100.00", "fund-a").await?;
    let replies = all((0..50).map(|n| {
        let api = api.clone();
        async move {
            operate(
                &api,
                "p2p_transfer.v1",
                &format!("spend-{n}"),
                transfer("2010:cust-a", "2010:cust-b", etb("10.00")),
            )
            .await
        }
    }))
    .await?;
    assert_eq!(replies.iter().filter(|r| r.status == 201).count(), 10);
    for reply in replies.iter().filter(|r| r.status != 201) {
        assert_eq!(reply.code(), "insufficient_funds", "{}", reply.body);
    }
    assert_eq!(balance(&api, &a).await?, "0.00");
    assert_eq!(balance(&api, &b).await?, "100.00");
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// AC-06.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac06_a_hold_ends_once_however_many_try_at_once() -> Result<(), BoxError> {
    let Some((server, api)) = server().await? else {
        return Ok(());
    };
    open(&api, OPERATOR, "1010").await?;
    let a = open(&api, CLIENT, "2010:cust-a").await?;
    let merchant = open(&api, CLIENT, "2030:merch-coffee").await?;
    deposit(&api, "2010:cust-a", "100.00", "fund-a").await?;
    for round in 0..5 {
        open(&api, CLIENT, &format!("2010:cust-hold-{round:04}")).await?;
        let hold = api
            .post(
                "/v1/holds",
                CLIENT,
                &format!("hold-{round}"),
                json!({"account": "2010:cust-a", "held_account": format!("2010:cust-hold-{round:04}"),
                       "amount": etb("20.00"), "expires_at": "2099-01-01T00:00:00Z"}),
            )
            .await?;
        assert_eq!(hold.status, 201, "{}", hold.body);
        let id = hold.str("/id");
        let replies = all((0..20).map(|n| {
            let (api, id) = (api.clone(), id.clone());
            async move {
                let (action, body) = if n % 2 == 0 {
                    (
                        "capture",
                        json!({"destination": "2030:merch-coffee", "amount": etb("15.00")}),
                    )
                } else {
                    ("release", json!({}))
                };
                api.post(
                    &format!("/v1/holds/{id}/{action}"),
                    CLIENT,
                    &format!("end-{round}-{n}"),
                    body,
                )
                .await
            }
        }))
        .await?;
        assert_eq!(
            replies.iter().filter(|r| r.status == 200).count(),
            1,
            "round {round}: exactly one ends the hold"
        );
        for reply in replies.iter().filter(|r| r.status != 200) {
            assert_eq!(
                (reply.status, reply.code()),
                (409, "hold_not_active"),
                "{}",
                reply.body
            );
        }
    }
    // Whatever won each round, the money adds up: 100.00 between the wallet
    // and the merchant, nothing left held.
    let wallet = balance(&api, &a).await?.parse::<f64>()?;
    let paid = balance(&api, &merchant).await?.parse::<f64>()?;
    assert!((wallet + paid - 100.00).abs() < 0.001, "{wallet} + {paid}");
    assert!(((paid / 15.00).round() * 15.00 - paid).abs() < 0.001);
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// AC-09.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac09_a_refund_cannot_probe_another_clients_entries() -> Result<(), BoxError> {
    let Some((server, api)) = server().await? else {
        return Ok(());
    };
    open(&api, OPERATOR, "1010").await?;
    open(&api, CLIENT, "2010:cust-a").await?;
    open(&api, CLIENT, "2030:merch-coffee").await?;
    open(&api, OTHER, "2010:cust-oa").await?;
    open(&api, OTHER, "2030:merch-ob").await?;
    deposit(&api, "2010:cust-a", "100.00", "fund-a").await?;
    let payment = operate(
        &api,
        "merchant_payment.v1",
        "pay-1",
        json!({
        "accounts": {"payer_wallet": "2010:cust-a", "merchant_balance": "2030:merch-coffee"},
        "amounts": {"amount": etb("10.00")}, "metadata": {"channel": "qr"}}),
    )
    .await?;
    assert_eq!(payment.status, 201, "{}", payment.body);

    // The other client names the first client's payment, then an entry
    // that does not exist: the two answers are the same.
    let mut answers = Vec::new();
    for (key, entry) in [
        ("probe-1", payment.str("/id")),
        ("probe-2", "0192f3c4-0000-7000-8000-00000000dead".to_owned()),
    ] {
        let reply = api
            .post(
                "/v1/operations/merchant_refund.v1",
                OTHER,
                key,
                json!({"accounts": {"merchant_balance": "2030:merch-ob", "payer_wallet": "2010:cust-oa"},
                       "amounts": {"amount": etb("1.00")}, "refunds": entry}),
            )
            .await?;
        answers.push((
            reply.status,
            reply.code().to_owned(),
            reply.body["title"].clone(),
        ));
    }
    assert_eq!(answers[0], answers[1], "the answers tell the entries apart");
    assert_eq!(answers[0].0, 422);
    assert_eq!(answers[0].1, "refunded_entry_not_found");
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    mezgeb_core::chain::hex(bytes)
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// AC-07.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac07_one_voucher_per_device_and_each_receipt_pays_once() -> Result<(), BoxError> {
    let Some((server, api)) = server().await? else {
        return Ok(());
    };
    open(&api, OPERATOR, "1010").await?;
    let wallet = open(&api, CLIENT, "2010:cust-a").await?;
    let merchant = open(&api, CLIENT, "2030:merch-coffee").await?;
    let mut admin = server.db.admin().await?;
    mezgeb_engine::set_tier_limits(
        &mut admin,
        CLIENT_ID.parse()?,
        Tier::C,
        Some(Tier::C.maximum()),
    )
    .await?;
    drop(admin);
    deposit(&api, "2010:cust-a", "500.00", "fund-a").await?;
    let phone = SecretKey::from_bytes(&Sha256::digest(b"abuse phone").into())?;
    let device = api
        .call(
            "POST",
            "/v1/offline/devices",
            Some(CLIENT),
            None,
            Some(json!({"owner_account": "2010:cust-a",
                        "public_key": hex(phone.public_key().compressed()),
                        "label": "phone", "attestation": {"type": "none"}})),
        )
        .await?;
    assert_eq!(device.status, 201, "{}", device.body);
    let device_id = device.str("/id");

    // Ten vouchers asked for at once, with ten keys: one is issued.
    let replies = all((0..10).map(|n| {
        let (api, device_id) = (api.clone(), device_id.clone());
        async move {
            api.post(
                "/v1/offline/vouchers",
                CLIENT,
                &format!("vch-{n}"),
                json!({"device": device_id, "amount": etb("100.00")}),
            )
            .await
        }
    }))
    .await?;
    let issued: Vec<_> = replies.iter().filter(|r| r.status == 201).collect();
    assert_eq!(issued.len(), 1, "one voucher per device");
    for reply in replies.iter().filter(|r| r.status != 201) {
        assert_eq!(reply.code(), "voucher_open", "{}", reply.body);
    }
    let voucher = issued[0].body.clone();
    assert_eq!(balance(&api, &wallet).await?, "400.00");

    // Three receipts, uploaded by twenty syncs at once, in any order.
    let mut prev: [u8; 32] =
        Sha256::digest(unhex(voucher["body"].as_str().ok_or("no body")?)).into();
    let mut signed = Vec::new();
    let now = mezgeb_api::wire::now().as_micros();
    for (n, minor) in [2_500i64, 1_000, 500].into_iter().enumerate() {
        let receipt = Receipt {
            device_id: device_id.parse()?,
            voucher_id: voucher["id"].as_str().ok_or("no voucher")?.parse()?,
            counter: n as u64 + 1,
            prev_digest: prev,
            payee_account_id: merchant.parse()?,
            payee_device_id: None,
            amount: Money::from_minor(minor, Currency::ETB),
            event_time: Timestamp::from_micros(now - 60_000_000),
            payee_challenge: [n as u8 + 1; 16],
        };
        prev = receipt.digest();
        signed.push(json!({"body": hex(&receipt.body()), "signature": hex(&receipt.sign(&phone))}));
    }
    let syncs = all((0..20).map(|n| {
        let api = api.clone();
        let mut items = signed.clone();
        items.rotate_left(n % 3);
        async move {
            api.call(
                "POST",
                "/v1/offline/receipts",
                Some(CLIENT),
                None,
                Some(json!({"receipts": items})),
            )
            .await
        }
    }))
    .await?;
    for sync in &syncs {
        assert_eq!(sync.status, 200, "{}", sync.body);
    }
    let settled = syncs
        .iter()
        .flat_map(|s| s.body["results"].as_array().cloned().unwrap_or_default())
        .filter(|r| r["outcome"] == "SETTLED")
        .count();
    assert!(settled <= 3, "{settled} receipts settled");
    assert_eq!(balance(&api, &merchant).await?, "40.00");
    let escrow = voucher["escrow_account"].as_str().ok_or("no escrow")?;
    assert_eq!(balance(&api, escrow).await?, "60.00");
    let mut verifier = server.db.verifier().await?;
    assert_eq!(mezgeb_engine::escrow_findings(&mut verifier).await?, vec![]);
    drop(verifier);
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}
