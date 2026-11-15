//! Reports: the trial balance at a period's end, now and at a time; ageing.

use crate::common::{Api, BoxError, OPERATOR};

pub async fn trial_balance_and_ageing(api: &Api, period: i64) -> Result<(), BoxError> {
    let at_close = expect!(
        api.get(
            &format!("/v1/reports/trial-balance?period_id={period}"),
            OPERATOR
        )
        .await?,
        200
    );
    assert_eq!(at_close.body["balanced"], true);
    let now = expect!(api.get("/v1/reports/trial-balance", OPERATOR).await?, 200);
    assert_eq!(now.body["rows"], at_close.body["rows"]);
    expect!(
        api.get(
            "/v1/reports/trial-balance?as_of=2100-01-01T00:00:00Z",
            OPERATOR
        )
        .await?,
        200
    );
    expect!(api.get("/v1/reports/ageing", OPERATOR).await?, 200);
    expect!(
        api.get("/v1/reports/ageing?code=2060&code=9030", OPERATOR)
            .await?,
        200
    );
    Ok(())
}
