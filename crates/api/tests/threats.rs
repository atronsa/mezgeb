//! The threats of `docs/threat-model.md` that the step 2.08 exit gate maps
//! to the API, each tested as the attacker would try it: a compromised
//! client server holding a real client credential, and an insider holding
//! an operator credential. The evidence file maps every "compromised
//! client" and "insider" threat to its tests, here and elsewhere.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Api, BoxError, CLIENT, Contract, OPERATOR, TestServer, etb};
use serde_json::json;

macro_rules! expect {
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

async fn server() -> Result<Option<(TestServer, Api)>, BoxError> {
    let Some(server) = TestServer::start(Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    let api = Api::new(&server.base, contract);
    Ok(Some((server, api)))
}

/// TM-07: a compromised client tries to create money. It cannot post raw
/// lines, cannot bind a system account to a role that does not allow it
/// (crediting a wallet from fee revenue), and cannot open system accounts.
#[tokio::test]
async fn tm07_a_client_cannot_create_money() -> Result<(), BoxError> {
    let Some((server, api)) = server().await? else {
        return Ok(());
    };
    for reference in ["1010", "4010"] {
        let opened = api
            .post(
                "/v1/accounts",
                OPERATOR,
                &format!("open-{reference}"),
                json!({"reference": reference}),
            )
            .await?;
        assert_eq!(opened.status, 201, "{}", opened.body);
    }
    let wallet = api
        .post(
            "/v1/accounts",
            CLIENT,
            "open-w",
            json!({"reference": "2010:cust-w"}),
        )
        .await?;
    assert_eq!(wallet.status, 201, "{}", wallet.body);

    // No raw entries: the only endpoint taking lines is the operator's.
    expect!(
        api.post(
            "/v1/adjustments",
            CLIENT,
            "mint",
            json!({"reason": "goodwill_credit",
                   "lines": [{"account": "4010", "side": "debit", "amount": etb("1000.00")},
                             {"account": "2010:cust-w", "side": "credit", "amount": etb("1000.00")}]})
        )
        .await?,
        403,
        "forbidden"
    );
    // Fee revenue as the payer of a transfer: the template's role allows
    // only wallets.
    expect!(
        api.post(
            "/v1/operations/p2p_transfer.v1",
            CLIENT,
            "from-fees",
            json!({"accounts": {"payer_wallet": "4010", "payee_wallet": "2010:cust-w"},
                   "amounts": {"amount": etb("1000.00")}})
        )
        .await?,
        422,
        "account_not_allowed"
    );
    // Nor can it open a system account of its own to bind.
    expect!(
        api.post(
            "/v1/accounts",
            CLIENT,
            "open-3010",
            json!({"reference": "3010"})
        )
        .await?,
        403,
        "forbidden"
    );
    let balance = api
        .get(
            &format!("/v1/accounts/{}/balance", wallet.str("/id")),
            CLIENT,
        )
        .await?;
    assert_eq!(balance.str("/balance/amount"), "0.00");
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// TM-13: an insider tries to reopen a closed period. There is no such
/// operation, in the contract or in the service; closing again with the
/// same label is refused. The database refuses the change too
/// (`posting_into_a_closed_period_is_refused`, engine tests).
#[tokio::test]
async fn tm13_a_closed_period_cannot_be_reopened() -> Result<(), BoxError> {
    let Some((server, api)) = server().await? else {
        return Ok(());
    };
    // The contract has no operation that reopens or changes a period.
    let paths = api.contract.spec["paths"].as_object().ok_or("no paths")?;
    for (path, item) in paths {
        if path.starts_with("/v1/periods") {
            let methods: Vec<&String> = item
                .as_object()
                .map(|o| o.keys().collect())
                .unwrap_or_default();
            let allowed = if path == "/v1/periods/close" {
                "post"
            } else {
                "get"
            };
            assert!(
                methods.iter().all(|m| m.as_str() == allowed),
                "{path} has {methods:?}"
            );
        }
        assert!(!path.contains("reopen"), "{path}");
    }
    let current = api.get("/v1/periods/current", OPERATOR).await?;
    let period = current.body["id"].as_i64().ok_or("no period")?;
    let closed = api
        .call(
            "POST",
            "/v1/periods/close",
            Some(OPERATOR),
            None,
            Some(json!({"next_label": "p2"})),
        )
        .await?;
    assert_eq!(closed.status, 200, "{}", closed.body);
    // The service has no reopen route either.
    let reopen = api
        .http
        .post(format!("{}/v1/periods/{period}/reopen", server.base))
        .bearer_auth(OPERATOR)
        .send()
        .await?;
    assert_eq!(reopen.status().as_u16(), 404);
    let closed = api.get(&format!("/v1/periods/{period}"), OPERATOR).await?;
    assert_eq!(closed.str("/status"), "closed");
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}
