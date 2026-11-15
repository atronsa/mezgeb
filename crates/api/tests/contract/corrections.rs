//! Corrections: reversals, and adjustments by two people.

use serde_json::json;

use crate::common::{Api, BoxError, CHECKER, CLIENT, OPERATOR, etb};

use crate::operations::Posted;

pub async fn reversals_and_adjustments(api: &Api, posted: &Posted) -> Result<(), BoxError> {
    let p2p_id = posted.p2p.clone();
    // The P2P transfer cannot be reversed: Hana has spent part of it, and a
    // reversal never overdraws (ADR-0013). The payment request can.
    expect!(
        api.post(
            &format!("/v1/entries/{p2p_id}/reversal"),
            OPERATOR,
            "rev-0",
            json!({"reason": "booking_error"})
        )
        .await?,
        422,
        "insufficient_funds"
    );
    let p2p_id = posted.settled.clone();
    let reversal = expect!(
        api.post(
            &format!("/v1/entries/{p2p_id}/reversal"),
            OPERATOR,
            "rev-1",
            json!({"reason": "booking_error"})
        )
        .await?,
        201
    );
    assert_eq!(reversal.body["authorization"]["requested_by"], "ops:maker");
    expect!(
        api.post(
            &format!("/v1/entries/{p2p_id}/reversal"),
            OPERATOR,
            "rev-2",
            json!({"reason": "booking_error"})
        )
        .await?,
        409,
        "already_reversed"
    );
    expect!(
        api.post(
            &format!("/v1/entries/{p2p_id}/reversal"),
            CLIENT,
            "rev-3",
            json!({"reason": "booking_error"})
        )
        .await?,
        403,
        "forbidden"
    );

    let adjustment_body = json!({
        "reason": "goodwill_credit",
        "lines": [
            {"account": "5010", "side": "debit", "amount": etb("10.00")},
            {"account": "2010:cust-hana", "side": "credit", "amount": etb("10.00")}],
        "metadata": {"ticket": "CS-1"}});
    let requested = expect!(
        api.post(
            "/v1/adjustments",
            OPERATOR,
            "adj-1",
            adjustment_body.clone()
        )
        .await?,
        202
    );
    assert_eq!(requested.str("/status"), "pending");
    let adjustment = requested.str("/id");
    expect!(
        api.get(&format!("/v1/adjustments/{adjustment}"), OPERATOR)
            .await?,
        200
    );
    expect!(
        api.post(
            &format!("/v1/adjustments/{adjustment}/approval"),
            OPERATOR,
            "apr-1",
            json!({})
        )
        .await?,
        403,
        "maker_is_checker"
    );
    let approved = expect!(
        api.post(
            &format!("/v1/adjustments/{adjustment}/approval"),
            CHECKER,
            "apr-2",
            json!({})
        )
        .await?,
        201
    );
    assert_eq!(approved.str("/kind"), "adjustment");
    assert_eq!(approved.body["authorization"]["approved_by"], "ops:checker");
    let second = expect!(
        api.post("/v1/adjustments", OPERATOR, "adj-2", adjustment_body)
            .await?,
        202
    )
    .str("/id");
    let rejected = expect!(
        api.post(
            &format!("/v1/adjustments/{second}/rejection"),
            CHECKER,
            "rej-1",
            json!({"note": "no evidence"})
        )
        .await?,
        200
    );
    assert_eq!(rejected.str("/status"), "rejected");
    expect!(
        api.post(
            &format!("/v1/adjustments/{second}/approval"),
            CHECKER,
            "apr-3",
            json!({})
        )
        .await?,
        409,
        "adjustment_not_pending"
    );
    Ok(())
}
