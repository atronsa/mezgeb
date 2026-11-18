//! Reconciliation (step 2.15): a client's cache compared, a stale cache
//! cleared, breaks listed, read, explained and refused, runs listed.

use mezgeb_core::{Currency, Timestamp};
use mezgeb_engine::recon::{
    Mapping, MatchOptions, format_date, import_statement, parse_statement, reconcile_account,
};
use serde_json::json;

use crate::common::{Api, BoxError, CLIENT, OPERATOR, OTHER, TestServer, etb};
use crate::operate;

const DAY: i64 = 86_400_000_000;

fn now_micros() -> Result<i64, BoxError> {
    Ok(i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_micros(),
    )?)
}

pub async fn caches_breaks_and_runs(api: &Api, server: &TestServer) -> Result<(), BoxError> {
    // Abebe's wallet as the client's cache sees it: one santim off.
    let compare = |amount: &str| json!({"balances": [{"account": "2010:cust-abebe", "balance": etb(amount)}]});
    let first = expect!(
        api.post(
            "/v1/reconciliation/balances",
            CLIENT,
            "recon-1",
            compare("0.01")
        )
        .await?,
        200
    );
    assert_eq!(first.body["results"][0]["agrees"], false);
    assert_eq!(first.body["run"]["kind"], "internal");
    assert_eq!(first.body["run"]["breaks_new"], 1);
    let ledger = first.str("/results/0/ledger/amount");

    // The open break, as the operator sees it.
    let open = expect!(
        api.get("/v1/reconciliation/breaks?kind=stale_cache", OPERATOR)
            .await?,
        200
    );
    let id = open.str("/items/0/id");
    assert_eq!(open.str("/items/0/status"), "open");
    expect!(
        api.get(&format!("/v1/reconciliation/breaks/{id}"), OPERATOR)
            .await?,
        200
    );
    // A stale cache is not resolved by an entry; an unknown break is not
    // found.
    let resolve = json!({"entry_id": "0192f3c4-0000-7000-8000-000000000e09"});
    expect!(
        api.post(
            &format!("/v1/reconciliation/breaks/{id}/resolve"),
            OPERATOR,
            "recon-2",
            resolve.clone()
        )
        .await?,
        409,
        "break_not_closable"
    );
    expect!(
        api.post(
            "/v1/reconciliation/breaks/0192f3c4-0000-7000-8000-00000000dead/resolve",
            OPERATOR,
            "recon-2b",
            resolve
        )
        .await?,
        404
    );

    // Refreshed from the ledger, the cache agrees: the break clears.
    let refreshed = expect!(
        api.post(
            "/v1/reconciliation/balances",
            CLIENT,
            "recon-3",
            compare(&ledger)
        )
        .await?,
        200
    );
    assert_eq!(refreshed.body["results"][0]["agrees"], true);
    assert_eq!(refreshed.body["run"]["breaks_cleared"], 1);
    let cleared = expect!(
        api.get(&format!("/v1/reconciliation/breaks/{id}"), OPERATOR)
            .await?,
        200
    );
    assert_eq!(cleared.str("/status"), "cleared");
    expect!(
        api.post(
            &format!("/v1/reconciliation/breaks/{id}/explain"),
            OPERATOR,
            "recon-4",
            json!({"reason": "refreshed"})
        )
        .await?,
        409,
        "break_closed"
    );

    // Stale again; this time the operator explains it.
    expect!(
        api.post(
            "/v1/reconciliation/balances",
            CLIENT,
            "recon-5",
            compare("0.02")
        )
        .await?,
        200
    );
    let open = expect!(
        api.get("/v1/reconciliation/breaks?status=open&limit=1", OPERATOR)
            .await?,
        200
    );
    let id = open.str("/items/0/id");
    let explained = expect!(
        api.post(
            &format!("/v1/reconciliation/breaks/{id}/explain"),
            OPERATOR,
            "recon-7",
            json!({"reason": "the client's cache job was restarted"})
        )
        .await?,
        200
    );
    assert_eq!(explained.str("/status"), "explained");
    assert_eq!(
        explained.str("/note"),
        "the client's cache job was restarted"
    );

    // The bank shows 25.00 arriving tomorrow that the ledger does not have.
    // Its statement is imported and reconciled as `mezgeb-cli recon` does;
    // the ledger's own lines are older than the statement and not compared.
    let now = now_micros()?;
    let tomorrow = format_date(i32::try_from(now.div_euclid(DAY) + 1)?);
    let text = format!(
        "line_id,booking_date,amount,currency,reference
BANK-1,{tomorrow},25.00,ETB,UNKNOWN
"
    );
    let mut verifier = server.db.verifier().await?;
    let bank = mezgeb_engine::account_by_reference(&mut verifier, "1010")
        .await?
        .ok_or("no 1010")?
        .id();
    let lines = parse_statement(&text, &Mapping::mezgeb(), Currency::ETB)?;
    import_statement(
        &mut verifier,
        bank,
        &lines,
        "contract",
        Timestamp::from_micros(now),
    )
    .await?;
    let run = reconcile_account(
        &mut verifier,
        bank,
        MatchOptions::default(),
        Timestamp::from_micros(now + 4 * DAY),
    )
    .await?;
    assert_eq!(run.breaks_new, 1);
    drop(verifier);
    let money = expect!(
        api.get(
            "/v1/reconciliation/breaks?kind=missing_in_ledger&account=1010",
            OPERATOR
        )
        .await?,
        200
    );
    let id = money.str("/items/0/id");
    assert_eq!(money.str("/items/0/difference/amount"), "25.00");
    // Explaining a money break is refused.
    expect!(
        api.post(
            &format!("/v1/reconciliation/breaks/{id}/explain"),
            OPERATOR,
            "recon-10",
            json!({"reason": "probably fine"})
        )
        .await?,
        409,
        "break_not_closable"
    );
    // The client parks the 25.00 in suspense; the operator resolves the
    // break with that entry.
    crate::open(api, CLIENT, "9010:sus-0002").await?;
    let parked = expect!(
        operate(
            api,
            "deposit_unmatched.v1",
            "recon-suspense",
            json!({
                "accounts": {"customer_funds": "1010", "suspense_item": "9010:sus-0002"},
                "amounts": {"amount": etb("25.00")}})
        )
        .await?,
        201
    );
    let resolved = expect!(
        api.post(
            &format!("/v1/reconciliation/breaks/{id}/resolve"),
            OPERATOR,
            "recon-11",
            json!({"entry_id": parked.str("/id")})
        )
        .await?,
        200
    );
    assert_eq!(resolved.str("/status"), "resolved");
    assert_eq!(resolved.str("/resolution/entry"), parked.str("/id"));

    // Every break, by status.
    let all = expect!(
        api.get("/v1/reconciliation/breaks?status=all", OPERATOR)
            .await?,
        200
    );
    assert_eq!(all.body["items"].as_array().map(Vec::len), Some(3));
    expect!(
        api.get("/v1/reconciliation/breaks?status=closed", OPERATOR)
            .await?,
        400,
        "invalid_request"
    );

    // Only one's own accounts; balances in the account's currency.
    expect!(
        api.post(
            "/v1/reconciliation/balances",
            OTHER,
            "recon-8",
            compare("1.00")
        )
        .await?,
        404
    );
    expect!(
        api.post(
            "/v1/reconciliation/balances",
            CLIENT,
            "recon-9",
            json!({"balances": [{"account": "2010:cust-abebe",
                                 "balance": {"amount": "1.00", "currency": "USD"}}]})
        )
        .await?,
        400,
        "invalid_request"
    );

    let runs = expect!(
        api.get("/v1/reconciliation/runs?kind=internal", OPERATOR)
            .await?,
        200
    );
    assert_eq!(runs.body["items"].as_array().map(Vec::len), Some(3));
    expect!(
        api.get("/v1/reconciliation/runs?limit=0", OPERATOR).await?,
        400,
        "invalid_request"
    );
    Ok(())
}
