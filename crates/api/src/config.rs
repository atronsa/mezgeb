//! Service configuration, from the environment.
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `MEZGEB_DATABASE_URL` | required | PostgreSQL, as `mezgeb_app` |
//! | `MEZGEB_LISTEN` | `0.0.0.0:8080` | Address to listen on |
//! | `MEZGEB_CHART` | `charts/wallet-provider.toml` | The chart of accounts |
//! | `MEZGEB_TOKEN_TTL_SECONDS` | `900` | How long an access token lives |
//! | `MEZGEB_SIGNING_KEY_FILE` | required | The Ed25519 key that signs receipts and checkpoints (`mezgeb-cli key generate`) |
//! | `MEZGEB_REQUEST_TIMEOUT_MS` | `10000` | How long a request may take before the client is told to retry |
//! | `MEZGEB_BODY_LIMIT_BYTES` | `65536` | The largest request body accepted |
//! | `MEZGEB_DB_POOL` | `16` | Database connections |
//! | `MEZGEB_LOG` | `info` | Log filter (`tracing` syntax) |

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Everything the service needs to start.
#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub listen: SocketAddr,
    pub chart: PathBuf,
    pub token_ttl: Duration,
    pub signing_key: PathBuf,
    pub request_timeout: Duration,
    pub body_limit: usize,
    pub pool_size: u32,
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
            request_timeout: Duration::from_millis(number("MEZGEB_REQUEST_TIMEOUT_MS", 10_000)?),
            body_limit: number("MEZGEB_BODY_LIMIT_BYTES", 65_536)?,
            pool_size: number("MEZGEB_DB_POOL", 16)?,
        })
    }
}
