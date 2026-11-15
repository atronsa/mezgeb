//! Operations: every released template, replays, and the refusals.

use serde_json::{Value, json};

use crate::common::{Api, BoxError, CLIENT, etb};
use crate::operate;

use crate::accounts::Wallets;

/// The entries later steps look up and reverse.
pub struct Posted {
    pub p2p: String,
    pub settled: String,
}

pub async fn every_template(api: &Api, wallets: &Wallets) -> Result<Posted, BoxError> {
    let (abebe, coffee) = (wallets.abebe.clone(), wallets.coffee.clone());
    let deposit = expect!(
        operate(
            api,
            "deposit_confirmed.v1",
            "dep-1",
            json!({
        "accounts": {"customer_funds": "1010", "customer_wallet": "2010:cust-abebe"},
        "amounts": {"amount": etb("1000.00")}})
        )
        .await?,
        201
    );
    assert_eq!(deposit.str("/template"), "deposit_confirmed.v1");
    // The same request again: the same entry, replayed.
    let repeat = expect!(
        operate(
            api,
            "deposit_confirmed.v1",
            "dep-1",
            json!({
        "accounts": {"customer_funds": "1010", "customer_wallet": "2010:cust-abebe"},
        "amounts": {"amount": etb("1000.00")}})
        )
        .await?,
        200
    );
    assert!(repeat.replayed());
    assert_eq!(repeat.str("/id"), deposit.str("/id"));
    expect!(
        operate(
            api,
            "deposit_confirmed.v1",
            "dep-1",
            json!({
        "accounts": {"customer_funds": "1010", "customer_wallet": "2010:cust-abebe"},
        "amounts": {"amount": etb("999.00")}})
        )
        .await?,
        409,
        "idempotency_key_reused"
    );

    let p2p = expect!(
        operate(
            api,
            "p2p_transfer.v1",
            "p2p-1",
            json!({
        "accounts": {"payer_wallet": abebe, "payee_wallet": "2010:cust-hana"},
        "amounts": {"amount": etb("150.00")}})
        )
        .await?,
        201
    );
    let payment = expect!(operate(api, "merchant_payment.v1", "pay-1", json!({
        "accounts": {"payer_wallet": "2010:cust-abebe", "merchant_balance": coffee, "fee_revenue": "4010"},
        "amounts": {"amount": etb("100.00"), "merchant_fee": etb("2.00")},
        "metadata": {"channel": "qr"}})).await?, 201);
    let settled = expect!(
        operate(
            api,
            "payment_request_settle.v1",
            "req-1",
            json!({
        "accounts": {"payer_wallet": "2010:cust-hana", "payee": "2010:cust-abebe"},
        "amounts": {"amount": etb("80.00")},
        "metadata": {"payment_request": "req-0001"}})
        )
        .await?,
        201
    );
    expect!(
        operate(
            api,
            "deposit_unmatched.v1",
            "unm-1",
            json!({
        "accounts": {"customer_funds": "1010", "suspense_item": "9010:sus-0001"},
        "amounts": {"amount": etb("50.00")}})
        )
        .await?,
        201
    );
    expect!(
        operate(
            api,
            "suspense_resolve.v1",
            "res-1",
            json!({
        "accounts": {"suspense_item": "9010:sus-0001", "owner": "2010:cust-hana"},
        "amounts": {"amount": etb("50.00")}})
        )
        .await?,
        201
    );
    for (n, outcome) in [(1, "withdrawal_confirmed.v1"), (2, "withdrawal_failed.v1")] {
        expect!(operate(api, "withdrawal_request.v1", &format!("wd-{n}"), json!({
            "accounts": {"customer_wallet": "2010:cust-abebe", "pending_withdrawal": format!("2020:wd-000{n}")},
            "amounts": {"amount": etb("30.00")}})).await?, 201);
        expect!(operate(api, "withdrawal_sent.v1", &format!("wd-sent-{n}"), json!({
            "accounts": {"pending_withdrawal": format!("2020:wd-000{n}"), "withdrawal_in_transit": format!("9030:wdt-000{n}")},
            "amounts": {"amount": etb("30.00")}})).await?, 201);
        let accounts = if n == 1 {
            json!({"withdrawal_in_transit": "9030:wdt-0001", "customer_funds": "1010"})
        } else {
            json!({"withdrawal_in_transit": "9030:wdt-0002", "customer_wallet": "2010:cust-abebe"})
        };
        expect!(
            operate(
                api,
                outcome,
                &format!("wd-done-{n}"),
                json!({
            "accounts": accounts, "amounts": {"amount": etb("30.00")}})
            )
            .await?,
            201
        );
    }
    let refund = expect!(operate(api, "merchant_refund.v1", "ref-1", json!({
        "accounts": {"merchant_balance": "2030:merch-coffee", "fee_revenue": "4010", "payer_wallet": "2010:cust-abebe"},
        "amounts": {"amount": etb("40.00"), "fee_refund": etb("0.80")},
        "refunds": payment.str("/id")})).await?, 201);
    assert_eq!(refund.str("/kind"), "refund");
    for (n, decide, accounts) in [
        (
            1,
            "dispute_resolve_payer.v1",
            json!({"dispute_item": "2060:dsp-0001", "payer_wallet": "2010:cust-abebe"}),
        ),
        (
            2,
            "dispute_resolve_merchant.v1",
            json!({"dispute_item": "2060:dsp-0002", "merchant_balance": "2030:merch-coffee"}),
        ),
    ] {
        expect!(operate(api, "dispute_open.v1", &format!("dsp-{n}"), json!({
            "accounts": {"merchant_balance": "2030:merch-coffee", "dispute_item": format!("2060:dsp-000{n}")},
            "amounts": {"amount": etb("10.00")},
            "metadata": {"disputed_entry": payment.str("/id")}})).await?, 201);
        let mut body = json!({"accounts": accounts, "amounts": {"amount": etb("10.00")}});
        if n == 1 {
            body["refunds"] = Value::String(payment.str("/id"));
        }
        expect!(
            operate(api, decide, &format!("dsp-done-{n}"), body).await?,
            201
        );
    }
    Ok(Posted {
        p2p: p2p.str("/id"),
        settled: settled.str("/id"),
    })
}

/// Refusals, each with its code.
pub async fn refusals(api: &Api, wallets: &Wallets) -> Result<(), BoxError> {
    let coffee = wallets.coffee.clone();
    let insufficient = expect!(
        operate(
            api,
            "p2p_transfer.v1",
            "p2p-big",
            json!({
        "accounts": {"payer_wallet": "2010:cust-hana", "payee_wallet": "2010:cust-abebe"},
        "amounts": {"amount": etb("100000.00")}})
        )
        .await?,
        422,
        "insufficient_funds"
    );
    assert_eq!(insufficient.body["rule"], 1012);
    expect!(
        operate(
            api,
            "p2p_transfer.v1",
            "p2p-nobody",
            json!({
        "accounts": {"payer_wallet": "2010:cust-abebe", "payee_wallet": "2010:cust-nobody"},
        "amounts": {"amount": etb("1.00")}})
        )
        .await?,
        422,
        "bound_account_not_found"
    );
    expect!(
        operate(
            api,
            "merchant_payment.v1",
            "pay-nfc",
            json!({
        "accounts": {"payer_wallet": "2010:cust-abebe", "merchant_balance": coffee},
        "amounts": {"amount": etb("1.00")}, "metadata": {"channel": "nfc"}})
        )
        .await?,
        422,
        "invalid_metadata"
    );
    expect!(
        api.call(
            "POST",
            "/v1/operations/p2p_transfer.v1",
            Some(CLIENT),
            None,
            Some(json!({}))
        )
        .await?,
        400,
        "idempotency_key_required"
    );
    expect!(
        api.call(
            "POST",
            "/v1/operations/p2p_transfer.v1",
            None,
            Some("x"),
            Some(json!({}))
        )
        .await?,
        401,
        "unauthenticated"
    );
    expect!(
        api.post("/v1/periods/close", CLIENT, "x", json!({"next_label": "x"}))
            .await?,
        403,
        "forbidden"
    );
    Ok(())
}
