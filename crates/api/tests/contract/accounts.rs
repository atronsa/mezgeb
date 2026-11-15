//! Accounts: open, replay, duplicate, list, find, get; later, status, balance and history.

use serde_json::json;

use crate::common::{Api, BoxError, CLIENT, OPERATOR};
use crate::open;

/// The wallets later steps post to.
pub struct Wallets {
    pub abebe: String,
    pub hana: String,
    pub coffee: String,
}

pub async fn open_and_find(api: &Api) -> Result<Wallets, BoxError> {
    for reference in ["1010", "4010", "5010"] {
        open(api, OPERATOR, reference).await?;
    }
    let abebe = open(api, CLIENT, "2010:cust-abebe").await?;
    let hana = open(api, CLIENT, "2010:cust-hana").await?;
    let coffee = open(api, CLIENT, "2030:merch-coffee").await?;
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
        open(api, CLIENT, reference).await?;
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
    Ok(Wallets {
        abebe,
        hana,
        coffee,
    })
}

/// Once there are entries: freeze and unfreeze, balance now and then, and
/// the history in pages.
pub async fn status_balance_and_history(api: &Api, wallets: &Wallets) -> Result<(), BoxError> {
    let (abebe, hana) = (wallets.abebe.clone(), wallets.hana.clone());
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
    Ok(())
}
