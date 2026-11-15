//! Times: RFC 3339, in UTC, with microseconds.

use mezgeb_core::Timestamp;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::problem::ApiError;

/// RFC 3339, in UTC, with microseconds.
pub fn format_time(time: Timestamp) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(time.as_micros()) * 1_000)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

/// Parses an RFC 3339 time; `pointer` names the field in errors.
pub fn parse_time(text: &str, pointer: &str) -> Result<Timestamp, ApiError> {
    let time = OffsetDateTime::parse(text, &Rfc3339)
        .map_err(|_| ApiError::invalid(pointer, format!("{text:?} is not an RFC 3339 time")))?;
    let micros = time.unix_timestamp_nanos() / 1_000;
    i64::try_from(micros)
        .map(Timestamp::from_micros)
        .map_err(|_| ApiError::invalid(pointer, format!("{text:?} is out of range")))
}

/// The current time, from the service's clock.
pub fn now() -> Timestamp {
    let micros = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000;
    Timestamp::from_micros(i64::try_from(micros).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_round_trip_with_microseconds() -> Result<(), ApiError> {
        let t = parse_time("2026-11-14T09:30:12.418205Z", "/t")?;
        assert_eq!(format_time(t), "2026-11-14T09:30:12.418205Z");
        assert_eq!(
            parse_time("2026-11-14T12:30:12+03:00", "/t")?,
            parse_time("2026-11-14T09:30:12Z", "/t")?
        );
        assert!(parse_time("yesterday", "/t").is_err());
        Ok(())
    }
}
