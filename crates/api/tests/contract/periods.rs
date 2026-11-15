//! Periods: list, the open one, one by ID, and the close. A trial balance at the period's end is refused until it has closed.

use serde_json::json;

use crate::common::{Api, BoxError, CLIENT, OPERATOR};

/// Closes the open period; returns its ID.
pub async fn list_get_and_close(api: &Api) -> Result<i64, BoxError> {
    expect!(api.get("/v1/periods", CLIENT).await?, 200);
    let current = expect!(api.get("/v1/periods/current", CLIENT).await?, 200);
    let period = current.body["id"].as_i64().unwrap_or(0);
    expect!(
        api.get(&format!("/v1/periods/{period}"), CLIENT).await?,
        200
    );
    expect!(
        api.get(
            &format!("/v1/reports/trial-balance?period_id={period}"),
            OPERATOR
        )
        .await?,
        409,
        "period_not_closed"
    );
    let closed = expect!(
        api.call(
            "POST",
            "/v1/periods/close",
            Some(OPERATOR),
            None,
            Some(json!({"next_label": "day-2"}))
        )
        .await?,
        200
    );
    assert_eq!(closed.body["period"]["status"], "closed");
    expect!(
        api.call(
            "POST",
            "/v1/periods/close",
            Some(OPERATOR),
            None,
            Some(json!({"next_label": "day-2"}))
        )
        .await?,
        409,
        "period_label_taken"
    );
    Ok(period)
}
