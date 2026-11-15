//! OAuth: a token for the wallet credential, used at once; then the
//! refusals, each with its RFC 6749 error.

use serde_json::Value;

use crate::common::{Api, BoxError, CLIENT_CREDENTIAL, CLIENT_SECRET};

pub async fn token_and_refusals(api: &Api) -> Result<(), BoxError> {
    let issued = api
        .token(
            Some((CLIENT_CREDENTIAL, CLIENT_SECRET)),
            "grant_type=client_credentials&scope=accounts%3Aread",
        )
        .await?;
    assert_eq!(issued.status, 200, "{}", issued.body);
    assert_eq!(issued.body["scope"], "accounts:read");
    assert_eq!(
        issued.headers.get("cache-control").map(|v| v.as_bytes()),
        Some(&b"no-store"[..])
    );
    let token = issued.str("/access_token");
    let listed = api.get("/v1/accounts", &token).await?;
    assert_eq!(listed.status, 200, "{}", listed.body);
    // The token has only the scope it asked for.
    let refused = api.get("/v1/templates", &token).await?;
    assert_eq!((refused.status, refused.code()), (403, "forbidden"));

    let error = |reply: &crate::common::Reply| reply.body["error"].clone();
    let wrong = api
        .token(
            Some((CLIENT_CREDENTIAL, "not-the-secret")),
            "grant_type=client_credentials",
        )
        .await?;
    assert_eq!(
        (wrong.status, error(&wrong)),
        (401, Value::from("invalid_client"))
    );
    let unknown = api
        .token(
            Some(("0192f3c4-0000-7000-8000-00000000dead", CLIENT_SECRET)),
            "grant_type=client_credentials",
        )
        .await?;
    assert_eq!(unknown.body, wrong.body, "unknown and wrong look the same");
    let grant = api
        .token(
            Some((CLIENT_CREDENTIAL, CLIENT_SECRET)),
            "grant_type=password",
        )
        .await?;
    assert_eq!(
        (grant.status, error(&grant)),
        (400, Value::from("unsupported_grant_type"))
    );
    let scope = api
        .token(
            Some((CLIENT_CREDENTIAL, CLIENT_SECRET)),
            "grant_type=client_credentials&scope=periods%3Aclose",
        )
        .await?;
    assert_eq!(
        (scope.status, error(&scope)),
        (400, Value::from("invalid_scope"))
    );
    Ok(())
}
