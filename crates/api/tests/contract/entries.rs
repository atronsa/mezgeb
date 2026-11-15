//! Entries: get one, find one by its key.

use crate::common::{Api, BoxError, CLIENT};

use crate::operations::Posted;

pub async fn lookups(api: &Api, posted: &Posted) -> Result<(), BoxError> {
    let p2p_id = posted.p2p.clone();
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
    Ok(())
}
