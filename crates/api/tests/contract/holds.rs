//! Holds: place, replay, get, capture, release.

use serde_json::json;

use crate::common::{Api, BoxError, CLIENT, etb};

pub async fn lifecycle(api: &Api) -> Result<(), BoxError> {
    let hold_body = json!({"account": "2010:cust-abebe", "held_account": "2010:cust-hold-0001",
                           "amount": etb("20.00"), "expires_at": "2099-01-01T00:00:00Z"});
    let hold = expect!(
        api.post("/v1/holds", CLIENT, "hold-1", hold_body.clone())
            .await?,
        201
    );
    let hold_id = hold.str("/id");
    let hold_again = expect!(
        api.post("/v1/holds", CLIENT, "hold-1", hold_body).await?,
        200
    );
    assert_eq!(hold_again.str("/id"), hold_id);
    expect!(api.get(&format!("/v1/holds/{hold_id}"), CLIENT).await?, 200);
    let captured = expect!(
        api.post(
            &format!("/v1/holds/{hold_id}/capture"),
            CLIENT,
            "cap-1",
            json!({"destination": "2030:merch-coffee", "amount": etb("15.00")})
        )
        .await?,
        200
    );
    assert_eq!(captured.str("/status"), "captured");
    expect!(
        api.post(
            &format!("/v1/holds/{hold_id}/release"),
            CLIENT,
            "rel-0",
            json!({})
        )
        .await?,
        409,
        "hold_not_active"
    );
    let second_hold = expect!(api.post("/v1/holds", CLIENT, "hold-2", json!({"account": "2010:cust-abebe",
        "held_account": "2010:cust-hold-0002", "amount": etb("5.00"), "expires_at": "2099-01-01T00:00:00Z"})).await?, 201).str("/id");
    let released = expect!(
        api.post(
            &format!("/v1/holds/{second_hold}/release"),
            CLIENT,
            "rel-1",
            json!({})
        )
        .await?,
        200
    );
    assert_eq!(released.str("/status"), "released");
    Ok(())
}
