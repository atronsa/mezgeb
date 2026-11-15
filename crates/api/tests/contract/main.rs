//! The contract tests (step 2.07): every operation of
//! `openapi/mezgeb.v1.yaml`, called on a running service, with every
//! response checked against the contract: a documented status, and a body
//! that matches the documented schema. The acceptance criterion is that
//! 100% of the contract's operations pass.
//!
//! One scenario on one server, in steps, one module per tag of the contract.
//! Each step uses what the earlier ones made (wallets, entries). At the end,
//! the test fails if any operation was never called.

#[path = "../common/mod.rs"]
mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Api, BoxError, CLIENT, Contract, TestServer};
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

mod accounts;
mod audit;
mod corrections;
mod entries;
mod holds;
mod oauth;
mod offline;
mod operations;
mod periods;
mod reports;
mod templates;

pub(crate) async fn open(api: &Api, token: &str, reference: &str) -> Result<String, BoxError> {
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

pub(crate) async fn operate(
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

    let wallets = accounts::open_and_find(&api).await?;
    templates::catalogue(&api).await?;
    let posted = operations::every_template(&api, &wallets).await?;
    operations::refusals(&api, &wallets).await?;
    entries::lookups(&api, &posted).await?;
    corrections::reversals_and_adjustments(&api, &posted).await?;
    holds::lifecycle(&api).await?;
    accounts::status_balance_and_history(&api, &wallets).await?;
    let period = periods::list_get_and_close(&api).await?;
    reports::trial_balance_and_ageing(&api, period).await?;
    offline::planned(&api).await?;
    oauth::token_and_refusals(&api).await?;
    audit::keys_checkpoints_and_events(&api).await?;

    // ------------------------------------------------------------ coverage
    let uncovered = contract.uncovered();
    assert!(
        uncovered.is_empty(),
        "operations no test called: {uncovered:?}"
    );
    assert_eq!(contract.operation_count(), 46, "operations in the contract");
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}
