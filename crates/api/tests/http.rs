//! The HTTP layer under stress (step 2.07): timeouts that do not cancel
//! writes, bad amounts and oversized bodies, and graceful shutdown under
//! load.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Api, BoxError, CLIENT, CLIENT_CREDENTIAL, Contract, OPERATOR, SLOW, TestServer, etb};
use serde_json::{Value, json};

async fn fund(api: &Api, wallets: &[&str]) -> Result<(), BoxError> {
    for reference in ["1010"] {
        api.post(
            "/v1/accounts",
            OPERATOR,
            &format!("open-{reference}"),
            json!({"reference": reference}),
        )
        .await?;
    }
    for wallet in wallets {
        let opened = api
            .post(
                "/v1/accounts",
                CLIENT,
                &format!("open-{wallet}"),
                json!({"reference": wallet}),
            )
            .await?;
        assert_eq!(opened.status, 201, "{}", opened.body);
        let deposit = api
            .post(
                "/v1/operations/deposit_confirmed.v1",
                CLIENT,
                &format!("fund-{wallet}"),
                json!({"accounts": {"customer_funds": "1010", "customer_wallet": wallet},
                       "amounts": {"amount": etb("100000.00")}}),
            )
            .await?;
        assert_eq!(deposit.status, 201, "{}", deposit.body);
    }
    Ok(())
}

fn transfer(amount: &str) -> Value {
    json!({"accounts": {"payer_wallet": "2010:cust-a", "payee_wallet": "2010:cust-b"},
           "amounts": {"amount": etb(amount)}})
}

/// A client times out while its posting is still waiting for a lock; the
/// posting still completes, and the retry with the same key returns that
/// one entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_timeout_then_a_retry_gives_one_entry() -> Result<(), BoxError> {
    // Fund the wallets on a server with the usual timeout...
    let Some(setup) = TestServer::start(Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    fund(
        &Api::new(&setup.base, contract.clone()),
        &["2010:cust-a", "2010:cust-b"],
    )
    .await?;
    // ...then serve the same database with a one-second timeout.
    let db = setup.into_db().await.map_err(|e| e.to_string())?;
    let server = TestServer::on(db, Duration::from_millis(1_000))
        .await
        .map_err(|e| e.to_string())?;
    let api = Api::new(&server.base, contract);

    // Hold the payer's row, so the posting waits for it.
    let mut blocker = server.db.app().await?;
    sqlx::query("BEGIN").execute(&mut blocker).await?;
    sqlx::query("SELECT 1 FROM accounts WHERE code = 2010 AND sub_account = 'cust-a' FOR UPDATE")
        .execute(&mut blocker)
        .await?;
    let first = api
        .post(
            "/v1/operations/p2p_transfer.v1",
            CLIENT,
            "slow-1",
            transfer("10.00"),
        )
        .await?;
    assert_eq!(
        (first.status, first.code()),
        (503, "busy"),
        "{}",
        first.body
    );
    assert!(first.headers.get("retry-after").is_some());

    // The client gave up, but the posting is still waiting, not cancelled.
    sqlx::query("ROLLBACK").execute(&mut blocker).await?;
    let mut entry = None;
    // Up to 30 seconds: the commit itself takes milliseconds, but a busy
    // machine running other tests beside this one can delay it.
    for _ in 0..600 {
        let found = api
            .get("/v1/entries/by-idempotency-key/slow-1", CLIENT)
            .await?;
        if found.status == 200 {
            entry = Some(found.str("/id"));
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let entry = entry.ok_or("the timed-out posting never committed")?;
    let retry = api
        .post(
            "/v1/operations/p2p_transfer.v1",
            CLIENT,
            "slow-1",
            transfer("10.00"),
        )
        .await?;
    assert_eq!(retry.status, 200, "{}", retry.body);
    assert!(retry.replayed());
    assert_eq!(retry.str("/id"), entry);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM journal_entries WHERE idempotency_key = 'slow-1'")
            .fetch_one(&mut blocker)
            .await?;
    assert_eq!(count, 1);
    drop(blocker);
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Bad amounts and oversized bodies are refused with their codes, and
/// nothing is posted.
#[tokio::test]
async fn bad_amounts_and_huge_payloads_are_refused() -> Result<(), BoxError> {
    let Some(server) = TestServer::start(Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    let api = Api::new(&server.base, contract);
    fund(&api, &["2010:cust-a", "2010:cust-b"]).await?;
    let post = |key: &'static str, body: Value| {
        let api = api.clone();
        async move {
            api.post("/v1/operations/p2p_transfer.v1", CLIENT, key, body)
                .await
        }
    };

    for (key, amount) in [
        ("m1", "10.255"),
        ("m2", "ten"),
        ("m3", "1e3"),
        ("m4", "10,00"),
        ("m5", ""),
    ] {
        let reply = post(key, transfer(amount)).await?;
        assert_eq!(
            (reply.status, reply.code()),
            (400, "invalid_request"),
            "{amount:?}: {}",
            reply.body
        );
        assert_eq!(reply.body["errors"][0]["pointer"], "/amounts/amount/amount");
    }
    let negative = post("neg", transfer("-5.00")).await?;
    assert_eq!(
        (negative.status, negative.code()),
        (422, "non_positive_amount"),
        "{}",
        negative.body
    );
    assert_eq!(negative.body["rule"], 2006);
    let zero = post("zero", transfer("0.00")).await?;
    assert_eq!(zero.code(), "non_positive_amount");
    let float = post(
        "float",
        json!({"accounts": {"payer_wallet": "2010:cust-a", "payee_wallet": "2010:cust-b"},
               "amounts": {"amount": {"amount": 10.5, "currency": "ETB"}}}),
    )
    .await?;
    assert_eq!(
        (float.status, float.code()),
        (400, "invalid_request"),
        "{}",
        float.body
    );
    let currency = post(
        "currency",
        json!({"accounts": {"payer_wallet": "2010:cust-a", "payee_wallet": "2010:cust-b"},
               "amounts": {"amount": {"amount": "1.00", "currency": "XYZ"}}}),
    )
    .await?;
    assert_eq!((currency.status, currency.code()), (400, "invalid_request"));
    let unknown_field = post(
        "field",
        json!({"accounts": {"payer_wallet": "2010:cust-a", "payee_wallet": "2010:cust-b"},
               "amounts": {"amount": etb("1.00")}, "lines": []}),
    )
    .await?;
    assert_eq!(
        (unknown_field.status, unknown_field.code()),
        (400, "invalid_request")
    );
    let huge = post(
        "huge",
        json!({"accounts": {"payer_wallet": "2010:cust-a", "payee_wallet": "2010:cust-b"},
               "amounts": {"amount": etb("1.00")},
               "metadata": {"padding": "x".repeat(200_000)}}),
    )
    .await?;
    assert_eq!(
        (huge.status, huge.code()),
        (400, "invalid_request"),
        "{}",
        huge.body
    );

    let posted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM journal_entries WHERE idempotency_key NOT LIKE 'fund-%'",
    )
    .fetch_one(&mut server.db.app().await?)
    .await?;
    assert_eq!(posted, 0, "nothing refused was posted");
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Shutting down while clients post: every request the service
/// acknowledged is in the journal, once.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn graceful_shutdown_under_load_loses_no_acknowledged_request() -> Result<(), BoxError> {
    let Some(mut server) = TestServer::start(Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    let api = Api::new(&server.base, contract);
    fund(&api, &["2010:cust-a", "2010:cust-b"]).await?;

    let workers: Vec<_> = (0..8)
        .map(|w| {
            let http = reqwest::Client::new();
            let base = server.base.clone();
            tokio::spawn(async move {
                let mut acknowledged = Vec::new();
                for n in 0..10_000 {
                    let key = format!("load-{w}-{n}");
                    let sent = http
                        .post(format!("{base}/v1/operations/p2p_transfer.v1"))
                        .bearer_auth(CLIENT)
                        .header("Idempotency-Key", &key)
                        .json(&transfer("0.01"))
                        .send()
                        .await;
                    let Ok(response) = sent else { break };
                    let status = response.status().as_u16();
                    let Ok(body) = response.json::<Value>().await else {
                        break;
                    };
                    if status == 201 {
                        acknowledged.push((key, body["id"].as_str().unwrap_or("").to_owned()));
                    }
                }
                acknowledged
            })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(3_000)).await;
    server.stop().await.map_err(|e| e.to_string())?;
    let mut acknowledged = Vec::new();
    for worker in workers {
        acknowledged.extend(worker.await?);
    }
    assert!(
        acknowledged.len() >= 8,
        "only {} postings before the shutdown",
        acknowledged.len()
    );

    let mut conn = server.db.app().await?;
    for (key, id) in &acknowledged {
        let stored: Vec<uuid::Uuid> =
            sqlx::query_scalar("SELECT id FROM journal_entries WHERE idempotency_key = $1")
                .bind(key)
                .fetch_all(&mut conn)
                .await?;
        assert_eq!(stored.len(), 1, "{key}");
        assert_eq!(stored[0].to_string(), *id, "{key}");
    }
    assert!(mezgeb_engine::integrity_report(&mut conn).await?.is_clean());
    drop(conn);
    server.db.drop().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Authorization comes first: a caller without the scope gets `403`
/// whatever else is wrong with its request, so a refusal never reveals how
/// far a request would have got.
#[tokio::test]
async fn a_missing_scope_is_refused_before_the_request_is_read() -> Result<(), BoxError> {
    let Some(server) = TestServer::start(Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    let api = Api::new(&server.base, contract);
    // The wallet client has none of these scopes. Each request is also
    // broken in another way, which would be a 400 for a caller allowed to
    // make it.
    for (method, path, key, body) in [
        (
            "POST",
            "/v1/periods/close",
            None,
            Some(json!({"next_label": 7})),
        ),
        ("POST", "/v1/adjustments", None, Some(json!({"lines": "x"}))),
        ("POST", "/v1/adjustments", Some("k"), None),
        (
            "POST",
            "/v1/entries/not-an-id/reversal",
            None,
            Some(json!([])),
        ),
        (
            "GET",
            "/v1/reports/trial-balance?as_of=yesterday&x=1",
            None,
            None,
        ),
        (
            "POST",
            "/v1/accounts/not-an-id/status",
            None,
            Some(json!({})),
        ),
    ] {
        let reply = api.call(method, path, Some(CLIENT), key, body).await?;
        assert_eq!(
            (reply.status, reply.code()),
            (403, "forbidden"),
            "{method} {path}: {}",
            reply.body
        );
    }
    // The same broken request from a caller with the scope is a 400.
    let allowed = api
        .call(
            "POST",
            "/v1/periods/close",
            Some(OPERATOR),
            None,
            Some(json!({"next_label": 7})),
        )
        .await?;
    assert_eq!(allowed.status, 400, "{}", allowed.body);
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Rate limits (TM-03): a client over its limit is refused with `429` and
/// `Retry-After`, other clients are not slowed, and the token endpoint
/// limits attempts per credential before hashing anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clients_and_logins_are_rate_limited() -> Result<(), BoxError> {
    let Some(server) = TestServer::start(Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    let api = Api::new(&server.base, contract);

    // The slow client: its burst of 3 passes, then it is refused. On a busy
    // machine a request can take long enough for the bucket to refill one,
    // so send until refused rather than exactly four.
    let mut passed = 0;
    let mut limited = None;
    for _ in 0..20 {
        let reply = api.get("/v1/periods/current", SLOW).await?;
        if reply.status == 429 {
            limited = Some(reply);
            break;
        }
        assert_eq!(reply.status, 200, "{}", reply.body);
        passed += 1;
    }
    assert!(
        passed >= 3,
        "only {passed} requests passed before the limit"
    );
    let limited = limited.ok_or("the slow client was never limited")?;
    assert_eq!(limited.code(), "rate_limited", "{}", limited.body);
    assert_eq!(
        limited.headers.get("retry-after").map(|v| v.as_bytes()),
        Some(&b"1"[..])
    );
    // Another client is not slowed down by it.
    let other = api.get("/v1/periods/current", CLIENT).await?;
    assert_eq!(other.status, 200);
    // After Retry-After, the slow client may go on.
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let again = api.get("/v1/periods/current", SLOW).await?;
    assert_eq!(again.status, 200, "{}", again.body);

    // Guessing a secret: many attempts at once, most refused before any
    // hashing.
    let attempts = (0..30).map(|_| {
        let api = api.clone();
        async move {
            api.token(
                Some((CLIENT_CREDENTIAL, "a-guess")),
                "grant_type=client_credentials",
            )
            .await
        }
    });
    let replies = futures_join_all(attempts).await?;
    let slowed = replies.iter().filter(|r| r.status == 429).count();
    let refused = replies.iter().filter(|r| r.status == 401).count();
    assert_eq!(slowed + refused, 30);
    assert!(refused <= 12, "{refused} attempts were hashed");
    assert!(
        replies
            .iter()
            .filter(|r| r.status == 429)
            .all(|r| r.body["error"] == "slow_down" && r.headers.contains_key("retry-after"))
    );
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Runs the futures concurrently and collects their results in order.
async fn futures_join_all<F, T>(futures: impl Iterator<Item = F>) -> Result<Vec<T>, BoxError>
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
