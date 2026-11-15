//! The contract tests (step 2.07): every operation of
//! `openapi/mezgeb.v1.yaml`, called on a running service, with every
//! response checked against the contract: a documented status, and a body
//! that matches the documented schema. The acceptance criterion is that
//! 100% of the contract's operations pass.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Api, BoxError, CHECKER, CLIENT, Contract, OPERATOR, TestServer, etb};
use serde_json::{Value, json};

/// Asserts a status, with the body in the message.
macro_rules! expect {
    ($reply:expr, $status:expr) => {{
        let reply = $reply;
        assert_eq!(reply.status, $status, "{}", reply.body);
        reply
    }};
    ($reply:expr, $status:expr, $code:expr) => {{
        let reply = $reply;
        assert_eq!(
            (reply.status, reply.code()),
            ($status, $code),
            "{}",
            reply.body
        );
        reply
    }};
}

async fn open(api: &Api, token: &str, reference: &str) -> Result<String, BoxError> {
    let reply = expect!(
        api.post(
            "/v1/accounts",
            token,
            &format!("open-{reference}"),
            json!({"reference": reference})
        )
        .await?,
        201
    );
    Ok(reply.str("/id"))
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_operation_meets_the_contract() -> Result<(), BoxError> {
    let Some(server) = TestServer::start(Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    let api = Api::new(&server.base, contract.clone());

    // ------------------------------------------------------------ accounts
    for reference in ["1010", "4010", "5010"] {
        open(&api, OPERATOR, reference).await?;
    }
    let abebe = open(&api, CLIENT, "2010:cust-abebe").await?;
    let hana = open(&api, CLIENT, "2010:cust-hana").await?;
    let coffee = open(&api, CLIENT, "2030:merch-coffee").await?;
    for reference in [
        "2020:wd-0001",
        "9030:wdt-0001",
        "2020:wd-0002",
        "9030:wdt-0002",
        "9010:sus-0001",
        "2060:dsp-0001",
        "2060:dsp-0002",
        "2010:cust-hold-0001",
        "2010:cust-hold-0002",
    ] {
        open(&api, CLIENT, reference).await?;
    }
    // A retry returns the same account; another key for it is a duplicate.
    let again = expect!(
        api.post(
            "/v1/accounts",
            CLIENT,
            "open-2010:cust-abebe",
            json!({"reference": "2010:cust-abebe"})
        )
        .await?,
        200
    );
    assert!(again.replayed());
    assert_eq!(again.str("/id"), abebe);
    expect!(
        api.post(
            "/v1/accounts",
            CLIENT,
            "other-key",
            json!({"reference": "2010:cust-abebe"})
        )
        .await?,
        409,
        "duplicate_reference"
    );
    expect!(
        api.post(
            "/v1/accounts",
            CLIENT,
            "bad-ref",
            json!({"reference": "7777"})
        )
        .await?,
        422,
        "invalid_reference"
    );

    let listed = expect!(api.get("/v1/accounts?limit=5", CLIENT).await?, 200);
    assert_eq!(listed.body["items"].as_array().map(Vec::len), Some(5));
    let next = listed.str("/next_cursor");
    expect!(
        api.get(&format!("/v1/accounts?limit=5&cursor={next}"), CLIENT)
            .await?,
        200
    );
    let found = expect!(
        api.get("/v1/accounts?reference=2010:cust-hana", CLIENT)
            .await?,
        200
    );
    assert_eq!(found.str("/items/0/id"), hana);
    expect!(
        api.get(&format!("/v1/accounts/{abebe}"), CLIENT).await?,
        200
    );
    expect!(
        api.get("/v1/accounts/0192f3c4-0000-7000-8000-00000000dead", CLIENT)
            .await?,
        404,
        "not_found"
    );
    expect!(
        api.get("/v1/accounts/not-an-id", CLIENT).await?,
        404,
        "not_found"
    );

    // ------------------------------------------------------------ templates
    let templates = expect!(api.get("/v1/templates", CLIENT).await?, 200);
    assert_eq!(
        templates.body.as_array().map(Vec::len),
        Some(mezgeb_ops::RELEASED.len())
    );
    expect!(
        api.get("/v1/templates/merchant_payment.v1", CLIENT).await?,
        200
    );
    expect!(
        api.get("/v1/templates/merchant_payment.v9", CLIENT).await?,
        404,
        "unknown_template_version"
    );

    // ------------------------------------------------------------ operations
    let deposit = expect!(
        operate(
            &api,
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
            &api,
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
            &api,
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
            &api,
            "p2p_transfer.v1",
            "p2p-1",
            json!({
        "accounts": {"payer_wallet": abebe, "payee_wallet": "2010:cust-hana"},
        "amounts": {"amount": etb("150.00")}})
        )
        .await?,
        201
    );
    let payment = expect!(operate(&api, "merchant_payment.v1", "pay-1", json!({
        "accounts": {"payer_wallet": "2010:cust-abebe", "merchant_balance": coffee, "fee_revenue": "4010"},
        "amounts": {"amount": etb("100.00"), "merchant_fee": etb("2.00")},
        "metadata": {"channel": "qr"}})).await?, 201);
    let settled = expect!(
        operate(
            &api,
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
            &api,
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
            &api,
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
        expect!(operate(&api, "withdrawal_request.v1", &format!("wd-{n}"), json!({
            "accounts": {"customer_wallet": "2010:cust-abebe", "pending_withdrawal": format!("2020:wd-000{n}")},
            "amounts": {"amount": etb("30.00")}})).await?, 201);
        expect!(operate(&api, "withdrawal_sent.v1", &format!("wd-sent-{n}"), json!({
            "accounts": {"pending_withdrawal": format!("2020:wd-000{n}"), "withdrawal_in_transit": format!("9030:wdt-000{n}")},
            "amounts": {"amount": etb("30.00")}})).await?, 201);
        let accounts = if n == 1 {
            json!({"withdrawal_in_transit": "9030:wdt-0001", "customer_funds": "1010"})
        } else {
            json!({"withdrawal_in_transit": "9030:wdt-0002", "customer_wallet": "2010:cust-abebe"})
        };
        expect!(
            operate(
                &api,
                outcome,
                &format!("wd-done-{n}"),
                json!({
            "accounts": accounts, "amounts": {"amount": etb("30.00")}})
            )
            .await?,
            201
        );
    }
    let refund = expect!(operate(&api, "merchant_refund.v1", "ref-1", json!({
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
        expect!(operate(&api, "dispute_open.v1", &format!("dsp-{n}"), json!({
            "accounts": {"merchant_balance": "2030:merch-coffee", "dispute_item": format!("2060:dsp-000{n}")},
            "amounts": {"amount": etb("10.00")},
            "metadata": {"disputed_entry": payment.str("/id")}})).await?, 201);
        let mut body = json!({"accounts": accounts, "amounts": {"amount": etb("10.00")}});
        if n == 1 {
            body["refunds"] = Value::String(payment.str("/id"));
        }
        expect!(
            operate(&api, decide, &format!("dsp-done-{n}"), body).await?,
            201
        );
    }

    // Refusals, each with its code.
    let insufficient = expect!(
        operate(
            &api,
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
            &api,
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
            &api,
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

    // ------------------------------------------------------------ entries
    let p2p_id = p2p.str("/id");
    expect!(
        api.get(&format!("/v1/entries/{p2p_id}"), CLIENT).await?,
        200
    );
    let by_key = expect!(
        api.get("/v1/entries/by-idempotency-key/p2p-1", CLIENT)
            .await?,
        200
    );
    assert_eq!(by_key.str("/id"), p2p_id);
    expect!(
        api.get("/v1/entries/by-idempotency-key/never-used", CLIENT)
            .await?,
        404,
        "not_found"
    );

    // ------------------------------------------------------------ corrections
    // The P2P transfer cannot be reversed: Hana has spent part of it, and a
    // reversal never overdraws (ADR-0013). The payment request can.
    expect!(
        api.post(
            &format!("/v1/entries/{p2p_id}/reversal"),
            OPERATOR,
            "rev-0",
            json!({"reason": "booking_error"})
        )
        .await?,
        422,
        "insufficient_funds"
    );
    let p2p_id = settled.str("/id");
    let reversal = expect!(
        api.post(
            &format!("/v1/entries/{p2p_id}/reversal"),
            OPERATOR,
            "rev-1",
            json!({"reason": "booking_error"})
        )
        .await?,
        201
    );
    assert_eq!(reversal.body["authorization"]["requested_by"], "ops:maker");
    expect!(
        api.post(
            &format!("/v1/entries/{p2p_id}/reversal"),
            OPERATOR,
            "rev-2",
            json!({"reason": "booking_error"})
        )
        .await?,
        409,
        "already_reversed"
    );
    expect!(
        api.post(
            &format!("/v1/entries/{p2p_id}/reversal"),
            CLIENT,
            "rev-3",
            json!({"reason": "booking_error"})
        )
        .await?,
        403,
        "forbidden"
    );

    let adjustment_body = json!({
        "reason": "goodwill_credit",
        "lines": [
            {"account": "5010", "side": "debit", "amount": etb("10.00")},
            {"account": "2010:cust-hana", "side": "credit", "amount": etb("10.00")}],
        "metadata": {"ticket": "CS-1"}});
    let requested = expect!(
        api.post(
            "/v1/adjustments",
            OPERATOR,
            "adj-1",
            adjustment_body.clone()
        )
        .await?,
        202
    );
    assert_eq!(requested.str("/status"), "pending");
    let adjustment = requested.str("/id");
    expect!(
        api.get(&format!("/v1/adjustments/{adjustment}"), OPERATOR)
            .await?,
        200
    );
    expect!(
        api.post(
            &format!("/v1/adjustments/{adjustment}/approval"),
            OPERATOR,
            "apr-1",
            json!({})
        )
        .await?,
        403,
        "maker_is_checker"
    );
    let approved = expect!(
        api.post(
            &format!("/v1/adjustments/{adjustment}/approval"),
            CHECKER,
            "apr-2",
            json!({})
        )
        .await?,
        201
    );
    assert_eq!(approved.str("/kind"), "adjustment");
    assert_eq!(approved.body["authorization"]["approved_by"], "ops:checker");
    let second = expect!(
        api.post("/v1/adjustments", OPERATOR, "adj-2", adjustment_body)
            .await?,
        202
    )
    .str("/id");
    let rejected = expect!(
        api.post(
            &format!("/v1/adjustments/{second}/rejection"),
            CHECKER,
            "rej-1",
            json!({"note": "no evidence"})
        )
        .await?,
        200
    );
    assert_eq!(rejected.str("/status"), "rejected");
    expect!(
        api.post(
            &format!("/v1/adjustments/{second}/approval"),
            CHECKER,
            "apr-3",
            json!({})
        )
        .await?,
        409,
        "adjustment_not_pending"
    );

    // ------------------------------------------------------------ holds
    let hold_body = json!({"account": "2010:cust-abebe", "held_account": "2010:cust-hold-0001",
                           "amount": etb("20.00"), "expires_at": "2099-01-01T00:00:00Z"});
    let hold = expect!(
        api.post("/v1/holds", CLIENT, "hold-1", hold_body.clone())
            .await?,
        201
    );
    let hold_id = hold.str("/id");
    let hold_again = expect!(
        api.post("/v1/holds", CLIENT, "hold-1", hold_body).await?,
        200
    );
    assert_eq!(hold_again.str("/id"), hold_id);
    expect!(api.get(&format!("/v1/holds/{hold_id}"), CLIENT).await?, 200);
    let captured = expect!(
        api.post(
            &format!("/v1/holds/{hold_id}/capture"),
            CLIENT,
            "cap-1",
            json!({"destination": "2030:merch-coffee", "amount": etb("15.00")})
        )
        .await?,
        200
    );
    assert_eq!(captured.str("/status"), "captured");
    expect!(
        api.post(
            &format!("/v1/holds/{hold_id}/release"),
            CLIENT,
            "rel-0",
            json!({})
        )
        .await?,
        409,
        "hold_not_active"
    );
    let second_hold = expect!(api.post("/v1/holds", CLIENT, "hold-2", json!({"account": "2010:cust-abebe",
        "held_account": "2010:cust-hold-0002", "amount": etb("5.00"), "expires_at": "2099-01-01T00:00:00Z"})).await?, 201).str("/id");
    let released = expect!(
        api.post(
            &format!("/v1/holds/{second_hold}/release"),
            CLIENT,
            "rel-1",
            json!({})
        )
        .await?,
        200
    );
    assert_eq!(released.str("/status"), "released");

    // ------------------------------------------------------------ accounts, again
    let frozen = expect!(
        api.post(
            &format!("/v1/accounts/{hana}/status"),
            OPERATOR,
            "st-1",
            json!({"status": "frozen", "reason": "fraud_review"})
        )
        .await?,
        200
    );
    assert_eq!(frozen.str("/status"), "frozen");
    expect!(
        api.post(
            &format!("/v1/accounts/{hana}/status"),
            OPERATOR,
            "st-2",
            json!({"status": "open", "reason": "cleared"})
        )
        .await?,
        200
    );
    let balance = expect!(
        api.get(&format!("/v1/accounts/{abebe}/balance"), CLIENT)
            .await?,
        200
    );
    let balance_then = expect!(
        api.get(
            &format!("/v1/accounts/{abebe}/balance?as_of=2000-01-01T00:00:00Z"),
            CLIENT
        )
        .await?,
        200
    );
    assert_eq!(balance_then.str("/balance/amount"), "0.00");
    assert_ne!(balance.str("/balance/amount"), "0.00");
    let ledger = expect!(api.get(&format!("/v1/accounts/{abebe}/ledger?from=2000-01-01T00:00:00Z&to=2100-01-01T00:00:00Z&limit=3"), CLIENT).await?, 200);
    assert_eq!(ledger.body["items"].as_array().map(Vec::len), Some(3));
    let next = ledger.str("/next_cursor");
    expect!(api.get(&format!("/v1/accounts/{abebe}/ledger?from=2000-01-01T00:00:00Z&to=2100-01-01T00:00:00Z&cursor={next}"), CLIENT).await?, 200);

    // ------------------------------------------------------------ periods, reports
    expect!(api.get("/v1/periods", CLIENT).await?, 200);
    let current = expect!(api.get("/v1/periods/current", CLIENT).await?, 200);
    let period = current.body["id"].as_i64().unwrap_or(0);
    expect!(
        api.get(&format!("/v1/periods/{period}"), CLIENT).await?,
        200
    );
    expect!(
        api.get(
            &format!("/v1/reports/trial-balance?period_id={period}"),
            OPERATOR
        )
        .await?,
        409,
        "period_not_closed"
    );
    let closed = expect!(
        api.call(
            "POST",
            "/v1/periods/close",
            Some(OPERATOR),
            None,
            Some(json!({"next_label": "day-2"}))
        )
        .await?,
        200
    );
    assert_eq!(closed.body["period"]["status"], "closed");
    expect!(
        api.call(
            "POST",
            "/v1/periods/close",
            Some(OPERATOR),
            None,
            Some(json!({"next_label": "day-2"}))
        )
        .await?,
        409,
        "period_label_taken"
    );
    let at_close = expect!(
        api.get(
            &format!("/v1/reports/trial-balance?period_id={period}"),
            OPERATOR
        )
        .await?,
        200
    );
    assert_eq!(at_close.body["balanced"], true);
    let now = expect!(api.get("/v1/reports/trial-balance", OPERATOR).await?, 200);
    assert_eq!(now.body["rows"], at_close.body["rows"]);
    expect!(
        api.get(
            "/v1/reports/trial-balance?as_of=2100-01-01T00:00:00Z",
            OPERATOR
        )
        .await?,
        200
    );
    expect!(api.get("/v1/reports/ageing", OPERATOR).await?, 200);
    expect!(
        api.get("/v1/reports/ageing?code=2060&code=9030", OPERATOR)
            .await?,
        200
    );

    // ------------------------------------------------------------ offline (planned)
    for path in [
        "/v1/offline/devices",
        "/v1/offline/vouchers",
        "/v1/offline/receipts",
    ] {
        expect!(
            api.call("POST", path, Some(CLIENT), None, Some(json!({})))
                .await?,
            501,
            "not_implemented"
        );
    }

    // ------------------------------------------------------------ coverage
    let uncovered = contract.uncovered();
    assert!(
        uncovered.is_empty(),
        "operations no test called: {uncovered:?}"
    );
    assert_eq!(contract.operation_count(), 42, "operations in the contract");
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}
