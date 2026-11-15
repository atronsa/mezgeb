//! Authorization (step 2.08, ADR-0017): who may touch what.
//!
//! - one client cannot read or post against another client's accounts,
//!   entries or holds; it sees them exactly as if they did not exist (TM-05);
//! - system accounts are opened only by the admin, and bound by anyone where
//!   a template allows;
//! - an entry is visible to the client whose account it touches, even when
//!   the operator posted it.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    Api, BoxError, CHECKER, CLIENT, Contract, OPERATOR, OTHER, ROLE_TOKENS, TestServer, etb,
};
use serde_json::{Value, json};

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

fn role(name: &str) -> &'static str {
    ROLE_TOKENS
        .iter()
        .find(|(r, _)| *r == name)
        .map(|(_, t)| *t)
        .unwrap_or_default()
}

fn transfer(from: &str, to: &str) -> Value {
    json!({"accounts": {"payer_wallet": from, "payee_wallet": to},
           "amounts": {"amount": etb("1.00")}})
}

fn deposit(wallet: &str) -> Value {
    json!({"accounts": {"customer_funds": "1010", "customer_wallet": wallet},
           "amounts": {"amount": etb("100.00")}})
}

#[tokio::test]
async fn a_client_cannot_touch_another_clients_accounts() -> Result<(), BoxError> {
    let Some(server) = TestServer::start(Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    let api = Api::new(&server.base, contract);

    // System accounts are opened by the admin only.
    expect!(
        api.post(
            "/v1/accounts",
            CLIENT,
            "open-4010",
            json!({"reference": "4010"})
        )
        .await?,
        403,
        "forbidden"
    );
    for reference in ["1010", "5010"] {
        expect!(
            api.post(
                "/v1/accounts",
                OPERATOR,
                &format!("open-{reference}"),
                json!({"reference": reference})
            )
            .await?,
            201
        );
    }
    // Client A has wallet a; client B (OTHER) has wallet b. Both fund their
    // own wallet from the system account 1010, which anyone may bind.
    let a = expect!(
        api.post(
            "/v1/accounts",
            CLIENT,
            "open-a",
            json!({"reference": "2010:cust-a"})
        )
        .await?,
        201
    )
    .str("/id");
    let b = expect!(
        api.post(
            "/v1/accounts",
            OTHER,
            "open-b",
            json!({"reference": "2010:cust-b"})
        )
        .await?,
        201
    )
    .str("/id");
    let a_deposit = expect!(
        api.post(
            "/v1/operations/deposit_confirmed.v1",
            CLIENT,
            "fund",
            deposit(&a)
        )
        .await?,
        201
    )
    .str("/id");
    // The same idempotency key from another client is another request.
    expect!(
        api.post(
            "/v1/operations/deposit_confirmed.v1",
            OTHER,
            "fund",
            deposit(&b)
        )
        .await?,
        201
    );

    // B cannot read A's account, by ID or by reference, nor its history.
    for path in [
        format!("/v1/accounts/{a}"),
        format!("/v1/accounts/{a}/balance"),
        format!("/v1/accounts/{a}/ledger?from=2000-01-01T00:00:00Z&to=2100-01-01T00:00:00Z"),
    ] {
        expect!(api.get(&path, OTHER).await?, 404, "not_found");
    }
    let found = expect!(
        api.get("/v1/accounts?reference=2010:cust-a", OTHER).await?,
        200
    );
    assert_eq!(found.body["items"], json!([]));
    let listed = expect!(api.get("/v1/accounts", OTHER).await?, 200);
    let ids: Vec<&str> = listed.body["items"]
        .as_array()
        .map(|items| items.iter().filter_map(|i| i["id"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(ids, [b.as_str()], "B lists only its own accounts");
    // Nor A's entries: not by ID, and A's keys are not B's.
    expect!(
        api.get(&format!("/v1/entries/{a_deposit}"), OTHER).await?,
        404,
        "not_found"
    );

    // B cannot post against A's wallet, as payer or as payee, by ID or by
    // reference: to B, the account does not exist.
    for (key, body) in [
        ("steal", transfer(&a, &b)),
        ("steal-ref", transfer("2010:cust-a", &b)),
        ("pay-into", transfer(&b, &a)),
    ] {
        expect!(
            api.post("/v1/operations/p2p_transfer.v1", OTHER, key, body)
                .await?,
            422,
            "bound_account_not_found"
        );
    }

    // Holds: B cannot read, capture or release A's hold, nor hold A's money.
    let a_held = expect!(
        api.post(
            "/v1/accounts",
            CLIENT,
            "open-a-hold",
            json!({"reference": "2010:cust-a-hold"})
        )
        .await?,
        201
    )
    .str("/id");
    let hold = expect!(
        api.post(
            "/v1/holds",
            CLIENT,
            "hold",
            json!({"account": a, "held_account": a_held, "amount": etb("5.00"),
                   "expires_at": "2099-01-01T00:00:00Z"})
        )
        .await?,
        201
    )
    .str("/id");
    expect!(
        api.get(&format!("/v1/holds/{hold}"), OTHER).await?,
        404,
        "not_found"
    );
    expect!(
        api.post(
            &format!("/v1/holds/{hold}/capture"),
            OTHER,
            "cap",
            json!({"destination": b, "amount": etb("5.00")})
        )
        .await?,
        404,
        "not_found"
    );
    expect!(
        api.post(
            &format!("/v1/holds/{hold}/release"),
            OTHER,
            "rel",
            json!({})
        )
        .await?,
        404,
        "not_found"
    );
    expect!(
        api.post(
            "/v1/holds",
            OTHER,
            "hold-a",
            json!({"account": a, "held_account": b, "amount": etb("5.00"),
                   "expires_at": "2099-01-01T00:00:00Z"})
        )
        .await?,
        422,
        "account_not_found"
    );
    // A still can, and nothing of B's attempts moved A's money.
    expect!(api.get(&format!("/v1/holds/{hold}"), CLIENT).await?, 200);
    let balance = expect!(
        api.get(&format!("/v1/accounts/{a}/balance"), CLIENT)
            .await?,
        200
    );
    assert_eq!(balance.str("/balance/amount"), "95.00");

    // The auditor and the operator read across clients; the auditor's
    // credential still cannot capture A's hold.
    expect!(
        api.get(&format!("/v1/accounts/{a}"), role("auditor"))
            .await?,
        200
    );
    expect!(
        api.get(&format!("/v1/holds/{hold}"), role("auditor"))
            .await?,
        200
    );
    expect!(
        api.get(&format!("/v1/entries/{a_deposit}"), OPERATOR)
            .await?,
        200
    );

    // An entry the operator posts to A's wallet is visible to A, not to B.
    let requested = expect!(
        api.post(
            "/v1/adjustments",
            OPERATOR,
            "goodwill",
            json!({"reason": "goodwill_credit",
                   "lines": [{"account": "5010", "side": "debit", "amount": etb("2.00")},
                             {"account": a, "side": "credit", "amount": etb("2.00")}]})
        )
        .await?,
        202
    )
    .str("/id");
    let adjustment = expect!(
        api.post(
            &format!("/v1/adjustments/{requested}/approval"),
            CHECKER,
            "approve",
            json!({})
        )
        .await?,
        201
    )
    .str("/id");
    expect!(
        api.get(&format!("/v1/entries/{adjustment}"), CLIENT)
            .await?,
        200
    );
    expect!(
        api.get(&format!("/v1/entries/{adjustment}"), OTHER).await?,
        404,
        "not_found"
    );

    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// A path from the contract with its parameters filled in: IDs that exist
/// nowhere, so an allowed call answers something other than `403`.
fn concrete(path: &str) -> String {
    path.replace("{template_id}", "p2p_transfer.v1")
        .replace("{key}", "authz-none")
        .replace("{period_id}", "999999")
        .split('/')
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

/// The acceptance criterion of step 2.08: every operation of the contract,
/// called by a credential holding each single role, is allowed exactly when
/// the permission matrix grants one of the operation's scopes (and, for a
/// template, lists the template), and refused with `403 forbidden`
/// otherwise; called with no token, every one is `401`. The cases are
/// generated from `openapi/mezgeb.v1.yaml` and `crates/api/permissions.toml`,
/// so a new operation or a changed permission is covered without editing
/// this test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_role_gets_exactly_what_the_matrix_allows() -> Result<(), BoxError> {
    let Some(server) = TestServer::start(Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    let api = Api::new(&server.base, contract.clone());
    let permissions = mezgeb_api::auth::Permissions::built_in()?;

    // Every operation that takes a bearer token, with the scopes it accepts.
    let mut operations = Vec::new();
    let paths = contract.spec["paths"]
        .as_object()
        .ok_or("the contract has no paths")?;
    for (path, item) in paths {
        for (method, operation) in item.as_object().into_iter().flatten() {
            let requirements = operation["security"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let scopes: Vec<String> = requirements
                .iter()
                .filter_map(|r| r["oauth2"].as_array())
                .flatten()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect();
            if requirements
                .iter()
                .any(|r| r.get("clientSecretBasic").is_some())
            {
                continue; // The token endpoint: its own test, in contract/oauth.rs.
            }
            assert!(!scopes.is_empty(), "{method} {path} names no scope");
            operations.push((method.to_uppercase(), path.clone(), scopes));
        }
    }
    assert_eq!(operations.len(), 45, "operations with a bearer token");

    let mut cases = 0;
    for (n, (method, path, scopes)) in operations.iter().enumerate() {
        let url = concrete(path);
        let body = (method == "POST").then(|| json!({}));
        let key = format!("authz-{n}");
        // No token: always 401.
        let anonymous = api
            .call(method, &url, None, Some(&key), body.clone())
            .await?;
        assert_eq!(
            (anonymous.status, anonymous.code()),
            (401, "unauthenticated"),
            "{method} {path} without a token: {}",
            anonymous.body
        );
        for (role, token) in ROLE_TOKENS {
            let grant = &permissions.roles()[role];
            let template = path.strip_prefix("/v1/operations/");
            let allowed = scopes.iter().any(|s| grant.scopes.contains(s))
                && template.is_none_or(|t| grant.templates.contains(t));
            let reply = api
                .call(
                    method,
                    &url,
                    Some(token),
                    Some(&format!("{key}-{role}")),
                    body.clone(),
                )
                .await
                .map_err(|e| format!("{role} on {method} {url}: {e}"))?;
            let forbidden = reply.status == 403 && reply.code() == "forbidden";
            assert_eq!(
                !forbidden, allowed,
                "{role} on {method} {path}: {} {}",
                reply.status, reply.body
            );
            cases += 1;
        }
    }
    assert_eq!(cases, 45 * ROLE_TOKENS.len());
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// A revoked credential stops working at once: its existing token is
/// refused, and it cannot get a new one. Disabling a client does the same
/// for all of its credentials.
#[tokio::test]
async fn revoked_credentials_and_disabled_clients_stop_at_once() -> Result<(), BoxError> {
    let Some(server) = TestServer::start(Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    let api = Api::new(&server.base, contract);
    let mut admin = server.db.admin().await?;

    // A new credential of the wallet client, logged in through OAuth.
    let credential = mezgeb_engine::add_credential(
        &mut admin,
        uuid::Uuid::now_v7(),
        common::CLIENT_ID.parse()?,
        &mezgeb_core::ActorId::new("app:rotating")?,
        &["reader".to_owned()],
        common::secret_hash()?,
    )
    .await?;
    let id = credential.id.to_string();
    let login = || {
        api.token(
            Some((&id, common::CLIENT_SECRET)),
            "grant_type=client_credentials",
        )
    };
    let issued = expect!(login().await?, 200);
    let token = issued.str("/access_token");
    expect!(api.get("/v1/periods/current", &token).await?, 200);

    // Revoked: the token it already had is refused, replayed or not, and a
    // new one cannot be had.
    mezgeb_engine::revoke_credential(&mut admin, credential.id).await?;
    for _ in 0..2 {
        expect!(
            api.get("/v1/periods/current", &token).await?,
            401,
            "unauthenticated"
        );
    }
    let again = login().await?;
    assert_eq!(
        (again.status, again.body["error"].clone()),
        (401, json!("invalid_client"))
    );
    // The client's other credentials are not affected.
    expect!(api.get("/v1/periods/current", CLIENT).await?, 200);

    // Disabled: every credential of the client stops.
    expect!(api.get("/v1/periods/current", OTHER).await?, 200);
    mezgeb_engine::disable_client(&mut admin, common::OTHER_ID.parse()?).await?;
    expect!(
        api.get("/v1/periods/current", OTHER).await?,
        401,
        "unauthenticated"
    );
    expect!(api.get("/v1/periods/current", CLIENT).await?, 200);

    drop(admin);
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}
