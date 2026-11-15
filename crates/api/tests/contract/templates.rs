//! Templates: the catalogue, one template, an unknown version.

use crate::common::{Api, BoxError, CLIENT};

pub async fn catalogue(api: &Api) -> Result<(), BoxError> {
    let templates = expect!(api.get("/v1/templates", CLIENT).await?, 200);
    assert_eq!(
        templates.body.as_array().map(Vec::len),
        Some(mezgeb_ops::RELEASED.len())
    );
    expect!(
        api.get("/v1/templates/merchant_payment.v1", CLIENT).await?,
        200
    );
    expect!(
        api.get("/v1/templates/merchant_payment.v9", CLIENT).await?,
        404,
        "unknown_template_version"
    );
    Ok(())
}
