//! `mezgeb-cli offline`: the operator's side of offline payments (step
//! 2.11, ADR-0019).
//!
//! ```text
//! mezgeb-cli offline key generate --out FILE
//! mezgeb-cli offline tier CLIENT A|B|C --enable [--max ETB] [--lifetime-hours N]
//! mezgeb-cli offline tier CLIENT A|B|C --disable
//! mezgeb-cli offline device freeze|unfreeze|lost|revoke DEVICE_ID [--reason TEXT]
//! mezgeb-cli offline fee CLIENT --bps N
//! mezgeb-cli offline check
//! ```
//!
//! - `key generate` writes a new P-256 issuer key for
//!   `MEZGEB_OFFLINE_ISSUER_KEY_FILE`. It never overwrites a file.
//! - `tier` sets a client's limits for a tier, at or below the protocol's
//!   maxima, or disables it. Tier C (software keys) is off until enabled.
//!   Connect as `mezgeb_admin`.
//! - `device` moves any client's device: `freeze` for a conflict under
//!   review, `unfreeze` when it is resolved, `lost` when its owner reports
//!   it, `revoke` to retire it. Connect as `mezgeb_admin`.
//! - `fee` sets a client's merchant fee on offline payments, in basis points
//!   (200 is 2%, at most 1,000), taken from each settled receipt (E17).
//!   Without it, there is no fee. Connect as `mezgeb_admin`.
//! - `check` verifies that every escrow holds exactly its voucher's unspent
//!   value (exit status 1 if not). Connect as `mezgeb_verifier`.
//!
//! Every change by `tier`, `device` and `fee` is recorded as an audit event.

use std::fmt::Write as _;
use std::path::PathBuf;

use mezgeb_core::DeviceId;
use mezgeb_engine::{
    DeviceStatus, IssuerKey, escrow_findings, set_device_status, set_offline_fee, set_tier_limits,
};
use mezgeb_offline::{Tier, TierLimits};
use sqlx::PgConnection;

use crate::clients::{audit, client_named};

pub const USAGE: &str = "       mezgeb-cli offline key generate --out FILE\n       \
     mezgeb-cli offline tier CLIENT A|B|C --enable [--max ETB] [--lifetime-hours N] | --disable \
     [--database-url URL]\n       \
     mezgeb-cli offline device freeze|unfreeze|lost|revoke DEVICE_ID [--reason TEXT] \
     [--database-url URL]\n       \
     mezgeb-cli offline fee CLIENT --bps N [--database-url URL]\n       \
     mezgeb-cli offline check [--database-url URL]";

/// A parsed `offline` command.
#[derive(Debug, PartialEq, Eq)]
pub enum OfflineCommand {
    GenerateKey {
        out: PathBuf,
    },
    Tier {
        client: String,
        tier: Tier,
        /// `None` disables the tier.
        limits: Option<TierLimits>,
    },
    Device {
        device: DeviceId,
        to: DeviceStatus,
        reason: Option<String>,
    },
    Fee {
        client: String,
        bps: u32,
    },
    Check,
}

const HOUR_MICROS: i64 = 3_600_000_000;

/// ETB as written, `150` or `150.50`, in santim.
fn etb_minor(text: &str) -> Option<i64> {
    let (whole, cents) = match text.split_once('.') {
        Some((w, c)) if c.len() <= 2 => (w, format!("{c:0<2}")),
        Some(_) => return None,
        None => (text, "00".to_owned()),
    };
    let whole: i64 = whole.parse().ok()?;
    let cents: i64 = cents.parse().ok()?;
    whole.checked_mul(100)?.checked_add(cents)
}

/// Parses the words after `offline`. `--database-url` is taken out by the
/// caller.
pub fn parse(args: &[String]) -> Result<OfflineCommand, String> {
    let usage = || format!("usage:\n{USAGE}");
    let mut words = Vec::new();
    let (mut out, mut enable, mut disable, mut max, mut hours, mut reason) =
        (None, false, false, None, None, None);
    let mut bps = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().ok_or_else(usage)?)),
            "--enable" => enable = true,
            "--disable" => disable = true,
            "--max" => {
                let text = args.next().ok_or("--max needs an amount in ETB")?;
                max = Some(etb_minor(text).filter(|m| *m > 0).ok_or(format!(
                    "--max needs a positive amount in ETB, not {text:?}"
                ))?);
            }
            "--lifetime-hours" => {
                hours = Some(
                    args.next()
                        .and_then(|h| h.parse::<i64>().ok())
                        .filter(|h| *h >= 1)
                        .ok_or("--lifetime-hours needs a whole number of at least 1")?,
                );
            }
            "--reason" => reason = Some(args.next().ok_or("--reason needs a text")?.clone()),
            "--bps" => {
                bps = Some(
                    args.next()
                        .and_then(|b| b.parse::<u32>().ok())
                        .filter(|b| *b <= 1_000)
                        .ok_or("--bps needs a whole number from 0 to 1000")?,
                );
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument {other:?}\n{}", usage()));
            }
            other => words.push(other.to_owned()),
        }
    }
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["key", "generate"] => Ok(OfflineCommand::GenerateKey {
            out: out.ok_or(format!("key generate needs --out FILE\n{}", usage()))?,
        }),
        ["tier", client, tier] => {
            let tier: Tier = tier.parse().map_err(|e| format!("{e}\n{}", usage()))?;
            let limits = match (enable, disable) {
                (true, false) => {
                    let maximum = tier.maximum();
                    let limits = TierLimits {
                        max_voucher_minor: max.unwrap_or(maximum.max_voucher_minor),
                        lifetime_micros: hours.map_or(maximum.lifetime_micros, |h| h * HOUR_MICROS),
                    };
                    if !limits.within(tier) {
                        return Err(format!(
                            "tier {tier} allows at most {}.{:02} ETB and {} hours",
                            maximum.max_voucher_minor / 100,
                            maximum.max_voucher_minor % 100,
                            maximum.lifetime_micros / HOUR_MICROS
                        ));
                    }
                    Some(limits)
                }
                (false, true) if max.is_none() && hours.is_none() => None,
                _ => return Err(format!("tier needs --enable or --disable\n{}", usage())),
            };
            Ok(OfflineCommand::Tier {
                client: (*client).to_owned(),
                tier,
                limits,
            })
        }
        ["device", verb, id] => {
            let to = match *verb {
                "freeze" => DeviceStatus::Frozen,
                "unfreeze" => DeviceStatus::Active,
                "lost" => DeviceStatus::Lost,
                "revoke" => DeviceStatus::Revoked,
                _ => return Err(usage()),
            };
            Ok(OfflineCommand::Device {
                device: id.parse().map_err(|e| format!("device ID {id:?}: {e}"))?,
                to,
                reason,
            })
        }
        ["fee", client] => Ok(OfflineCommand::Fee {
            client: (*client).to_owned(),
            bps: bps.ok_or(format!("fee needs --bps N\n{}", usage()))?,
        }),
        ["check"] => Ok(OfflineCommand::Check),
        _ => Err(usage()),
    }
}

/// Writes a new issuer key to `out`, which must not exist.
pub fn generate(out: &std::path::Path) -> Result<String, String> {
    let scalar = IssuerKey::generate_hex().map_err(|e| e.to_string())?;
    let key = IssuerKey::from_hex(&scalar)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .map_err(|e| format!("cannot create {} (it must not exist): {e}", out.display()))?;
    std::io::Write::write_all(&mut file, format!("{scalar}\n").as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(format!(
        "wrote a new offline issuer key to {}: key ID {}. Keep the file secret.\n",
        out.display(),
        key.key_id()
    ))
}

/// Runs a command that needs the database. Returns the output and whether
/// all was clean.
pub async fn run(
    conn: &mut PgConnection,
    command: OfflineCommand,
) -> Result<(String, bool), String> {
    let mut out = String::new();
    match command {
        OfflineCommand::GenerateKey { out: path } => return Ok((generate(&path)?, true)),
        OfflineCommand::Tier {
            client,
            tier,
            limits,
        } => {
            let id = client_named(conn, &client).await?;
            set_tier_limits(conn, id, tier, limits)
                .await
                .map_err(|e| e.to_string())?;
            let after = match limits {
                Some(l) => format!(
                    r#"{{"tier":"{tier}","enabled":true,"max_voucher_minor":{},"lifetime_micros":{}}}"#,
                    l.max_voucher_minor, l.lifetime_micros
                ),
                None => format!(r#"{{"tier":"{tier}","enabled":false}}"#),
            };
            audit(
                conn,
                "offline.tier",
                Some(id),
                format!("client:{id}"),
                format!(r#"{{"after":{after}}}"#),
            )
            .await?;
            let _ = match limits {
                Some(l) => writeln!(
                    out,
                    "tier {tier} enabled for {client}: vouchers up to {}.{:02} ETB, for {} hours",
                    l.max_voucher_minor / 100,
                    l.max_voucher_minor % 100,
                    l.lifetime_micros / HOUR_MICROS
                ),
                None => writeln!(out, "tier {tier} disabled for {client}"),
            };
        }
        OfflineCommand::Device { device, to, reason } => {
            let changed = set_device_status(conn, None, device, to, reason.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            audit(
                conn,
                "offline.device",
                Some(changed.certificate.client_id),
                format!("device:{device}"),
                format!(r#"{{"after":{{"status":"{}"}}}}"#, to.as_str()),
            )
            .await?;
            let _ = writeln!(out, "device {device} is {}", changed.status.as_str());
        }
        OfflineCommand::Fee { client, bps } => {
            let id = client_named(conn, &client).await?;
            set_offline_fee(conn, id, bps)
                .await
                .map_err(|e| e.to_string())?;
            audit(
                conn,
                "offline.fee",
                Some(id),
                format!("client:{id}"),
                format!(r#"{{"after":{{"fee_bps":{bps}}}}}"#),
            )
            .await?;
            let _ = writeln!(
                out,
                "the offline merchant fee for {client} is {}.{:02}%",
                bps / 100,
                bps % 100
            );
        }
        OfflineCommand::Check => {
            let findings = escrow_findings(conn).await.map_err(|e| e.to_string())?;
            for f in &findings {
                let voucher = f
                    .voucher
                    .map_or_else(|| "no voucher".to_owned(), |v| format!("voucher {v}"));
                let _ = writeln!(
                    out,
                    "escrow {} ({voucher}) holds {} minor units; it should hold {}",
                    f.escrow_account, f.actual_minor, f.expected_minor
                );
            }
            if findings.is_empty() {
                let _ = writeln!(out, "every escrow holds its unspent voucher value");
            }
            return Ok((out, findings.is_empty()));
        }
    }
    Ok((out, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_offline_commands() -> Result<(), String> {
        assert_eq!(
            parse(&args(&["key", "generate", "--out", "issuer.hex"]))?,
            OfflineCommand::GenerateKey {
                out: "issuer.hex".into()
            }
        );
        assert_eq!(
            parse(&args(&["tier", "acme", "C", "--enable", "--max", "150.5"]))?,
            OfflineCommand::Tier {
                client: "acme".into(),
                tier: Tier::C,
                limits: Some(TierLimits {
                    max_voucher_minor: 15_050,
                    lifetime_micros: Tier::C.maximum().lifetime_micros
                })
            }
        );
        assert_eq!(
            parse(&args(&["tier", "acme", "B", "--disable"]))?,
            OfflineCommand::Tier {
                client: "acme".into(),
                tier: Tier::B,
                limits: None
            }
        );
        assert_eq!(
            parse(&args(&[
                "device",
                "freeze",
                "0192f3c4-0000-7000-8000-000000000d01",
                "--reason",
                "conflict"
            ]))?,
            OfflineCommand::Device {
                device: "0192f3c4-0000-7000-8000-000000000d01"
                    .parse()
                    .map_err(|e: mezgeb_core::IdError| e.to_string())?,
                to: DeviceStatus::Frozen,
                reason: Some("conflict".into())
            }
        );
        assert_eq!(parse(&args(&["check"]))?, OfflineCommand::Check);
        assert_eq!(
            parse(&args(&["fee", "acme", "--bps", "200"]))?,
            OfflineCommand::Fee {
                client: "acme".into(),
                bps: 200
            }
        );
        for bad in [
            &["tier", "acme", "C"][..],
            &["tier", "acme", "C", "--enable", "--max", "200.01"],
            &["tier", "acme", "A", "--enable", "--lifetime-hours", "169"],
            &["tier", "acme", "D", "--enable"],
            &["tier", "acme", "C", "--disable", "--max", "10"],
            &["device", "melt", "0192f3c4-0000-7000-8000-000000000d01"],
            &["device", "revoke", "not-a-uuid"],
            &["key", "generate"],
            &["check", "--full"],
            &["fee", "acme"],
            &["fee", "acme", "--bps", "1001"],
            &["fee", "acme", "--bps", "-1"],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
        Ok(())
    }

    #[test]
    fn amounts_are_read_in_etb() {
        assert_eq!(etb_minor("150"), Some(15_000));
        assert_eq!(etb_minor("150.5"), Some(15_050));
        assert_eq!(etb_minor("0.05"), Some(5));
        assert_eq!(etb_minor("1.005"), None);
        assert_eq!(etb_minor("abc"), None);
    }

    #[test]
    fn a_generated_issuer_key_is_never_overwritten() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("mezgeb-issuer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("issuer.hex");
        let _ = std::fs::remove_file(&path);
        generate(&path)?;
        let first = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        assert!(IssuerKey::from_hex(&first).is_ok());
        assert!(generate(&path).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).map_err(|e| e.to_string())?,
            first
        );
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(())
    }
}
