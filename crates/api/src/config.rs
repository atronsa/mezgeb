//! Service configuration, from the environment.
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `MEZGEB_DATABASE_URL` | required | PostgreSQL, as `mezgeb_app` |
//! | `MEZGEB_LISTEN` | `0.0.0.0:8080` | Address to listen on |
//! | `MEZGEB_CHART` | `charts/wallet-provider.toml` | The chart of accounts |
//! | `MEZGEB_TOKEN_TTL_SECONDS` | `900` | How long an access token lives |
//! | `MEZGEB_SIGNING_KEY_FILE` | required | The Ed25519 key that signs receipts and checkpoints (`mezgeb-cli key generate`) |
//! | `MEZGEB_OFFLINE_ISSUER_KEY_FILE` | none | The P-256 key that signs offline device certificates and vouchers (`mezgeb-cli key generate --offline-issuer`); without it, offline payments are off |
//! | `MEZGEB_OFFLINE_TICK_SECONDS` | `60` | How often voucher expiry, returns and gap timeouts run; `0` turns it off |
//! | `MEZGEB_REQUEST_TIMEOUT_MS` | `10000` | How long a request may take before the client is told to retry |
//! | `MEZGEB_BODY_LIMIT_BYTES` | `65536` | The largest request body accepted |
//! | `MEZGEB_DB_POOL` | `16` | Database connections |
//! | `MEZGEB_ADDRESS_FAILURE_LIMIT` | `0` | Failed logins and refused tokens per second per source address, in bursts of ten times that; `0` turns the limit off (ADR-0021) |
//! | `MEZGEB_ALLOW_DEVELOPMENT_KEY` | unset | `true` lets the service start with the published development signing key, `deploy/signing.dev.hex`; otherwise it refuses it |
//! | `MEZGEB_CLIENT_ADDRESS` | `peer` | Where the source address comes from: `peer` (the TCP connection) or `x-forwarded-for` (the last entry, added by the operator's own proxy; only when the service is reachable through it alone) |
//! | `MEZGEB_LOG` | `info` | Log filter (`tracing` syntax) |

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::auth::rate::{AddressLimit, AddressSource};

/// Everything the service needs to start.
#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub listen: SocketAddr,
    pub chart: PathBuf,
    pub token_ttl: Duration,
    pub signing_key: PathBuf,
    pub offline_issuer_key: Option<PathBuf>,
    pub offline_tick: Duration,
    pub request_timeout: Duration,
    pub body_limit: usize,
    pub pool_size: u32,
    pub address_limit: AddressLimit,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn number<T: std::str::FromStr>(name: &str, default: T) -> Result<T, String> {
    match var(name) {
        Some(value) => value
            .parse()
            .map_err(|_| format!("{name} must be a number, not {value:?}")),
        None => Ok(default),
    }
}

impl Config {
    /// Reads the configuration from the environment.
    pub fn from_env() -> Result<Config, String> {
        Ok(Config {
            database_url: var("MEZGEB_DATABASE_URL").ok_or("MEZGEB_DATABASE_URL is required")?,
            listen: var("MEZGEB_LISTEN")
                .unwrap_or_else(|| "0.0.0.0:8080".to_owned())
                .parse()
                .map_err(|_| "MEZGEB_LISTEN must be an address such as 0.0.0.0:8080")?,
            chart: var("MEZGEB_CHART")
                .unwrap_or_else(|| "charts/wallet-provider.toml".to_owned())
                .into(),
            token_ttl: Duration::from_secs(number("MEZGEB_TOKEN_TTL_SECONDS", 900)?),
            signing_key: var("MEZGEB_SIGNING_KEY_FILE")
                .ok_or("MEZGEB_SIGNING_KEY_FILE is required: every entry gets a signed receipt")?
                .into(),
            offline_issuer_key: var("MEZGEB_OFFLINE_ISSUER_KEY_FILE").map(PathBuf::from),
            offline_tick: Duration::from_secs(number("MEZGEB_OFFLINE_TICK_SECONDS", 60)?),
            request_timeout: Duration::from_millis(number("MEZGEB_REQUEST_TIMEOUT_MS", 10_000)?),
            body_limit: number("MEZGEB_BODY_LIMIT_BYTES", 65_536)?,
            pool_size: number("MEZGEB_DB_POOL", 16)?,
            address_limit: AddressLimit {
                per_second: number("MEZGEB_ADDRESS_FAILURE_LIMIT", 0)?,
                source: address_source(var("MEZGEB_CLIENT_ADDRESS").as_deref())?,
            },
        })
    }
}

/// The key ID of `deploy/signing.dev.hex`, the development signing key
/// published in the repository. Receipts and checkpoints signed with it can
/// be forged by anyone, so the service refuses it unless
/// `MEZGEB_ALLOW_DEVELOPMENT_KEY=true` says this is a development run
/// (step 2.16, TM-11).
pub const DEVELOPMENT_KEY_ID: &str = "d38f7234d5cb807e";

/// Refuses the published development signing key, unless allowed.
pub fn check_signing_key(key_id: &str) -> Result<(), String> {
    let allowed = var("MEZGEB_ALLOW_DEVELOPMENT_KEY").as_deref() == Some("true");
    if key_id == DEVELOPMENT_KEY_ID && !allowed {
        return Err(format!(
            "MEZGEB_SIGNING_KEY_FILE holds the development key {key_id}, which is published: \
             anyone could forge receipts and checkpoints. Generate a key with \
             `mezgeb-cli key generate`, or set MEZGEB_ALLOW_DEVELOPMENT_KEY=true on a \
             development machine."
        ));
    }
    Ok(())
}

fn address_source(value: Option<&str>) -> Result<AddressSource, String> {
    match value {
        None | Some("peer") => Ok(AddressSource::Peer),
        Some("x-forwarded-for") => Ok(AddressSource::ForwardedFor),
        Some(other) => Err(format!(
            "MEZGEB_CLIENT_ADDRESS must be peer or x-forwarded-for, not {other:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_source_is_the_peer_unless_named() {
        assert_eq!(address_source(None), Ok(AddressSource::Peer));
        assert_eq!(address_source(Some("peer")), Ok(AddressSource::Peer));
        assert_eq!(
            address_source(Some("x-forwarded-for")),
            Ok(AddressSource::ForwardedFor)
        );
        assert!(address_source(Some("X-Real-IP")).is_err());
    }

    #[test]
    fn the_development_key_id_is_the_published_keys() -> Result<(), String> {
        let signer =
            mezgeb_engine::Signer::from_hex(include_str!("../../../deploy/signing.dev.hex"))?;
        assert_eq!(signer.key_id(), DEVELOPMENT_KEY_ID);
        Ok(())
    }

    #[test]
    fn the_development_key_is_refused_and_others_are_not() {
        // MEZGEB_ALLOW_DEVELOPMENT_KEY is not set in the tests.
        assert!(check_signing_key(DEVELOPMENT_KEY_ID).is_err());
        assert!(check_signing_key("0123456789abcdef").is_ok());
    }
}
