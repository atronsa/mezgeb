//! Audit and tamper evidence through the API (step 2.09, ADR-0018): every
//! API mutation produces an audit event, every entry comes with a receipt
//! that verifies with the published key, a period closed through the API
//! gets a signed checkpoint, and the chains verify afterwards.

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use common::{
    Api, BoxError, CLIENT, CLIENT_CREDENTIAL, CLIENT_SECRET, Contract, OPERATOR, ROLE_TOKENS,
    TestServer, etb,
};
use mezgeb_core::chain::receipt_message;
use serde_json::{Value, json};

fn token_for(scopes: &[String]) -> Option<&'static str> {
    let permissions = mezgeb_api::auth::Permissions::built_in().ok()?;
    ROLE_TOKENS.iter().find_map(|(role, token)| {
        let grant = permissions.roles().get(*role)?;
        scopes
            .iter()
            .any(|s| grant.scopes.contains(s))
            .then_some(*token)
    })
}

fn auditor() -> &'static str {
    ROLE_TOKENS
        .iter()
        .find(|(role, _)| *role == "auditor")
        .map(|(_, token)| *token)
        .unwrap_or_default()
}

fn concrete(path: &str) -> String {
    path.split('/')
        .map(|part| {
            if part.starts_with('{') {
                "0192f3c4-0000-7000-8000-00000000dead"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn request_id(reply: &common::Reply) -> String {
    reply
        .headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

/// Every audit event, by request ID, once the ones written in the
/// background have landed (up to 10 s).
async fn events_with(api: &Api, wanted: &BTreeSet<String>) -> Result<Vec<Value>, BoxError> {
    let mut events = Vec::new();
    for _ in 0..100 {
        events.clear();
        let mut cursor = String::new();
        loop {
            let page = api
                .get(&format!("/v1/audit-events?limit=100{cursor}"), auditor())
                .await?;
            assert_eq!(page.status, 200, "{}", page.body);
            events.extend(page.body["items"].as_array().cloned().unwrap_or_default());
            match page.body["next_cursor"].as_str() {
                Some(next) => cursor = format!("&cursor={next}"),
                None => break,
            }
        }
        let seen: BTreeSet<String> = events
            .iter()
            .filter_map(|e| e["request_id"].as_str().map(str::to_owned))
            .collect();
        if wanted.is_subset(&seen) {
            return Ok(events);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(events)
}

/// The acceptance criterion: every API mutation produces an audit event.
/// Every write operation of the contract is called, by a role allowed to
/// make it, and its request ID must appear in the audit log; then real
/// changes, with their outcome and details.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_api_mutation_produces_an_audit_event() -> Result<(), BoxError> {
    let Some(server) = TestServer::start(Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    let api = Api::new(&server.base, contract.clone());

    // Every write operation of the contract, generated.
    let mut wanted = BTreeSet::new();
    let paths = contract.spec["paths"].as_object().ok_or("no paths")?;
    let mut writes = 0;
    for (path, item) in paths {
        for (method, operation) in item.as_object().into_iter().flatten() {
            if method == "get" || path == "/oauth/token" {
                continue;
            }
            let scopes: Vec<String> = operation["security"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| r["oauth2"].as_array())
                .flatten()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect();
            let token = token_for(&scopes).ok_or(format!("no role may {method} {path}"))?;
            let reply = api
                .call(
                    &method.to_uppercase(),
                    &concrete(path),
                    Some(token),
                    Some(&format!("audit-{writes}")),
                    Some(json!({})),
                )
                .await?;
            wanted.insert(request_id(&reply));
            writes += 1;
        }
    }
    assert_eq!(writes, 30, "write operations in the contract");

    // Real changes: one that succeeds, one refused by a rule, one without a
    // scope, one without a token, a change of configuration, and tokens.
    for reference in ["1010"] {
        let opened = api
            .post(
                "/v1/accounts",
                OPERATOR,
                &format!("open-{reference}"),
                json!({"reference": reference}),
            )
            .await?;
        wanted.insert(request_id(&opened));
    }
    let wallet = api
        .post(
            "/v1/accounts",
            CLIENT,
            "open-w",
            json!({"reference": "2010:cust-w"}),
        )
        .await?;
    wanted.insert(request_id(&wallet));
    let wallet = wallet.str("/id");
    let deposit = api
        .post(
            "/v1/operations/deposit_confirmed.v1",
            CLIENT,
            "dep",
            json!({"accounts": {"customer_funds": "1010", "customer_wallet": wallet},
                   "amounts": {"amount": etb("10.00")}}),
        )
        .await?;
    assert_eq!(deposit.status, 201, "{}", deposit.body);
    let refused = api
        .post(
            "/v1/operations/p2p_transfer.v1",
            CLIENT,
            "too-much",
            json!({"accounts": {"payer_wallet": wallet, "payee_wallet": "1010"},
                   "amounts": {"amount": etb("99.00")}}),
        )
        .await?;
    let forbidden = api
        .post("/v1/periods/close", CLIENT, "x", json!({}))
        .await?;
    let anonymous = api
        .call("POST", "/v1/accounts", None, Some("y"), Some(json!({})))
        .await?;
    let frozen = api
        .post(
            &format!("/v1/accounts/{wallet}/status"),
            OPERATOR,
            "freeze",
            json!({"status": "frozen", "reason": "fraud_review"}),
        )
        .await?;
    assert_eq!(frozen.status, 200, "{}", frozen.body);
    let token = api
        .token(
            Some((CLIENT_CREDENTIAL, CLIENT_SECRET)),
            "grant_type=client_credentials",
        )
        .await?;
    let bad_token = api
        .token(
            Some((CLIENT_CREDENTIAL, "guess")),
            "grant_type=client_credentials",
        )
        .await?;
    for reply in [
        &deposit, &refused, &forbidden, &anonymous, &frozen, &token, &bad_token,
    ] {
        wanted.insert(request_id(reply));
    }

    let events = events_with(&api, &wanted).await?;
    let by_request = |id: &str| -> Option<&Value> {
        events.iter().find(|e| e["request_id"].as_str() == Some(id))
    };
    for id in &wanted {
        assert!(by_request(id).is_some(), "no audit event for request {id}");
    }
    let outcome = |reply: &common::Reply| -> Value {
        by_request(&request_id(reply))
            .map(|e| e["outcome"].clone())
            .unwrap_or_default()
    };
    assert_eq!(outcome(&deposit), json!("ok"));
    assert_eq!(outcome(&refused), json!("refused"));
    assert_eq!(outcome(&forbidden), json!("forbidden"));
    assert_eq!(outcome(&anonymous), json!("unauthenticated"));
    assert_eq!(outcome(&bad_token), json!("unauthenticated"));
    let deposit_event = by_request(&request_id(&deposit)).ok_or("no deposit event")?;
    assert_eq!(
        deposit_event["action"],
        "POST /v1/operations/deposit_confirmed.v1"
    );
    assert_eq!(deposit_event["actor"], "app:wallet");
    assert_eq!(deposit_event["details"]["status"], 201);
    let refused_event = by_request(&request_id(&refused)).ok_or("no refused event")?;
    assert_eq!(refused_event["details"]["code"], "account_not_allowed");
    let frozen_event = by_request(&request_id(&frozen)).ok_or("no status event")?;
    assert_eq!(frozen_event["details"]["before"], json!({"status": "open"}));
    assert_eq!(
        frozen_event["details"]["after"],
        json!({"status": "frozen"})
    );
    let token_event = by_request(&request_id(&bad_token)).ok_or("no token event")?;
    assert_eq!(
        token_event["actor"],
        format!("credential:{CLIENT_CREDENTIAL}")
    );
    assert_eq!(token_event["details"]["code"], "invalid_client");
    // No audit event carries a secret.
    let all = serde_json::to_string(&events)?;
    assert!(!all.contains(CLIENT_SECRET) && !all.contains("guess"));
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Every entry comes with a receipt that verifies with the published key;
/// a period closed through the API gets a signed checkpoint, which a client
/// keeps; and the chains verify, against that copy too.
#[tokio::test]
async fn receipts_and_checkpoints_verify_end_to_end() -> Result<(), BoxError> {
    let Some(server) = TestServer::start(Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    let api = Api::new(&server.base, contract);
    api.post(
        "/v1/accounts",
        OPERATOR,
        "open-1010",
        json!({"reference": "1010"}),
    )
    .await?;
    let wallet = api
        .post(
            "/v1/accounts",
            CLIENT,
            "open-w",
            json!({"reference": "2010:cust-w"}),
        )
        .await?
        .str("/id");
    let mut entries = Vec::new();
    for n in 1..=3 {
        let posted = api
            .post(
                "/v1/operations/deposit_confirmed.v1",
                CLIENT,
                &format!("dep-{n}"),
                json!({"accounts": {"customer_funds": "1010", "customer_wallet": wallet},
                       "amounts": {"amount": etb("10.00")}}),
            )
            .await?;
        assert_eq!(posted.status, 201, "{}", posted.body);
        entries.push(posted.body);
    }
    // A retry returns the identical receipt (T-26b).
    let again = api
        .post(
            "/v1/operations/deposit_confirmed.v1",
            CLIENT,
            "dep-1",
            json!({"accounts": {"customer_funds": "1010", "customer_wallet": wallet},
                   "amounts": {"amount": etb("10.00")}}),
        )
        .await?;
    assert_eq!(again.body["receipt"], entries[0]["receipt"]);

    // Each receipt verifies with the key from /v1/keys alone.
    let keys = api.get("/v1/keys", CLIENT).await?;
    for entry in &entries {
        let receipt = &entry["receipt"];
        let key = keys.body["items"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|k| k["key_id"] == receipt["key_id"])
            .ok_or("the receipt's key is not published")?;
        let unhex = |s: &Value| -> Vec<u8> {
            let s = s.as_str().unwrap_or_default();
            (0..s.len())
                .step_by(2)
                .filter_map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
                .collect()
        };
        let hash: [u8; 32] = unhex(&receipt["hash"]).try_into().map_err(|_| "bad hash")?;
        let entry_id = receipt["entry"].as_str().ok_or("no entry")?.parse()?;
        let position = receipt["position"].as_u64().ok_or("no position")?;
        let message = receipt_message(position, entry_id, &hash);
        assert!(mezgeb_engine::signing::verify_signature(
            &unhex(&key["public_key"]),
            &message,
            &unhex(&receipt["signature"])
        ));
    }

    // Closing the period through the API signs a checkpoint.
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
    let checkpoints = api.get("/v1/checkpoints", CLIENT).await?;
    let line = checkpoints.str("/items/0/line");
    let copy = mezgeb_engine::Checkpoint::from_line(&line).ok_or("not a checkpoint line")?;
    assert_eq!(copy.position, 3);

    // The chains verify, against the client's copy too.
    let mut conn = server.db.verifier().await?;
    let report = mezgeb_engine::verify_chains(&mut conn, std::slice::from_ref(&copy)).await?;
    assert!(report.is_clean(), "{report}");
    assert_eq!((report.entries, report.unsigned), (3, 0));
    assert!(report.audit_events > 0);
    drop(conn);
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}
