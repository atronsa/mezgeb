//! Offline: the planned endpoints answer `501`.

use serde_json::json;

use crate::common::{Api, BoxError, CLIENT};

pub async fn planned(api: &Api) -> Result<(), BoxError> {
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
    Ok(())
}
