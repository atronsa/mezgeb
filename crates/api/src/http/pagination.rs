//! `limit` and `cursor` on list operations.

use serde::Deserialize;

use crate::problem::ApiError;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Page {
    pub(crate) limit: Option<i64>,
    pub(crate) cursor: Option<String>,
}

pub(crate) fn limit(page_limit: Option<i64>) -> Result<i64, ApiError> {
    match page_limit {
        None => Ok(50),
        Some(n) if (1..=100).contains(&n) => Ok(n),
        Some(_) => Err(ApiError::invalid(
            "/query/limit",
            "limit must be from 1 to 100",
        )),
    }
}
