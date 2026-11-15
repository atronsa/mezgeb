//! Audit: the signing keys, the checkpoint of the period closed earlier,
//! and the audit events of the scenario so far.

use crate::common::{Api, BoxError, ROLE_TOKENS};

pub async fn keys_checkpoints_and_events(api: &Api) -> Result<(), BoxError> {
    let auditor = ROLE_TOKENS
        .iter()
        .find(|(role, _)| *role == "auditor")
        .map(|(_, token)| *token)
        .ok_or("no auditor")?;
    let keys = api.get("/v1/keys", auditor).await?;
    assert_eq!(keys.status, 200, "{}", keys.body);
    assert_eq!(keys.body["items"].as_array().map(Vec::len), Some(1));
    let checkpoints = api.get("/v1/checkpoints?limit=10", auditor).await?;
    assert_eq!(checkpoints.status, 200, "{}", checkpoints.body);
    assert_eq!(
        checkpoints.body["items"].as_array().map(Vec::len),
        Some(1),
        "the period closed earlier has its checkpoint"
    );
    let events = api.get("/v1/audit-events?limit=5", auditor).await?;
    assert_eq!(events.status, 200, "{}", events.body);
    assert_eq!(events.body["items"].as_array().map(Vec::len), Some(5));
    let next = events.str("/next_cursor");
    let more = api
        .get(&format!("/v1/audit-events?limit=5&cursor={next}"), auditor)
        .await?;
    assert_eq!(more.status, 200, "{}", more.body);
    Ok(())
}
