//! Operator command line.
//!
//! ```text
//! mezgeb-cli verify [--full] [--settle-seconds N] [--no-record] [--database-url URL]
//! mezgeb-cli close-period --next LABEL [--database-url URL]
//! mezgeb-cli migrate [--database-url URL]
//! mezgeb-cli client create|list|disable|seed ... [--database-url URL]
//! mezgeb-cli credential issue|list|revoke ... [--database-url URL]
//! mezgeb-cli offline key|tier|device|check ... [--database-url URL]
//! ```
//!
//! `verify` proves that the stored balances equal the journal
//! (docs/db-schema.md#verification). Connect as `mezgeb_verifier`.
//!
//! `close-period` closes the open accounting period and opens the next one,
//! named `LABEL`; if a close stopped part-way, it finishes that one instead
//! (docs/runbooks/close-period.md). Connect as `mezgeb_app`.
//!
//! `migrate` applies the schema migrations that have not run yet. Connect
//! as `mezgeb_owner`; run it before starting a new version of the service.
//!
//! `client` and `credential` manage who may call the ledger: clients, their
//! credentials and their roles ([`clients`], ADR-0017). Connect as
//! `mezgeb_admin`.
//!
//! The URL comes from `--database-url` or `MEZGEB_DATABASE_URL`.
//!
//! Exit status: 0 clean (or closed), 1 problems found (the period stays
//! closing), 2 could not run.
//!
//! `verify --chain`, `checkpoint export` and `key generate` are tamper
//! evidence ([`chain`], ADR-0018). `close-period` signs a checkpoint of the
//! chains for the period it closed, with the key in `--signing-key` or
//! `MEZGEB_SIGNING_KEY_FILE` (or `--no-checkpoint`, for tests).
//!
//! `offline` sets up offline payments: the issuer key, each client's tier
//! limits, and device freezes and revocations ([`offline`], ADR-0019).
//!
//! Planned: `import` (step 3.14).

mod chain;
mod clients;
mod offline;

use std::process::ExitCode;
use std::time::Duration;

use mezgeb_engine::{MIGRATOR, Scope, VerifyOptions, close_period, verify};
use sqlx::{Connection, PgConnection};

const USAGE: &str = "usage: mezgeb-cli verify [--full] [--settle-seconds N] [--no-record] \
                     [--database-url URL]\n       \
                     mezgeb-cli verify --chain [--checkpoints DIR] [--database-url URL]\n       \
                     mezgeb-cli close-period --next LABEL [--signing-key FILE | --no-checkpoint] \
                     [--database-url URL]\n       \
                     mezgeb-cli migrate [--database-url URL]";

/// The parsed `verify` command.
#[derive(Debug, PartialEq, Eq)]
struct VerifyCommand {
    options_scope: Scope,
    settle: Duration,
    record: bool,
    /// Verify the hash chains instead of the balances.
    chain: bool,
    /// Checkpoint copies kept outside the database, for `--chain`.
    checkpoints: Option<std::path::PathBuf>,
    url: Option<String>,
}

/// A parsed command.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Verify(VerifyCommand),
    ClosePeriod {
        next: String,
        /// The key that signs the checkpoint; `None` with `--no-checkpoint`.
        signing_key: Option<std::path::PathBuf>,
        url: Option<String>,
    },
    Chain {
        command: chain::ChainCommand,
        url: Option<String>,
    },
    Migrate {
        url: Option<String>,
    },
    Client {
        command: clients::ClientCommand,
        url: Option<String>,
    },
    Offline {
        command: offline::OfflineCommand,
        url: Option<String>,
    },
}

fn database_url(args: &mut std::slice::Iter<'_, String>) -> Result<String, String> {
    Ok(args.next().ok_or("--database-url needs a URL")?.clone())
}

fn parse(args: &[String]) -> Result<Command, String> {
    let mut args = args.iter();
    match args.next().map(String::as_str) {
        Some("verify") => parse_verify(args),
        Some("close-period") => {
            let mut next = None;
            let mut url = None;
            let mut signing_key = std::env::var("MEZGEB_SIGNING_KEY_FILE")
                .ok()
                .filter(|v| !v.is_empty())
                .map(std::path::PathBuf::from);
            let mut no_checkpoint = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--next" => next = Some(args.next().ok_or("--next needs a label")?.clone()),
                    "--signing-key" => {
                        signing_key = Some(args.next().ok_or("--signing-key needs a file")?.into());
                    }
                    "--no-checkpoint" => no_checkpoint = true,
                    "--database-url" => url = Some(database_url(&mut args)?),
                    other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
                }
            }
            let next = next.ok_or(format!("close-period needs --next LABEL\n{USAGE}"))?;
            if no_checkpoint {
                signing_key = None;
            } else if signing_key.is_none() {
                return Err(format!(
                    "close-period signs a checkpoint: pass --signing-key FILE or set \
                     MEZGEB_SIGNING_KEY_FILE (or --no-checkpoint)\n{USAGE}"
                ));
            }
            Ok(Command::ClosePeriod {
                next,
                signing_key,
                url,
            })
        }
        Some("migrate") => {
            let mut url = None;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--database-url" => url = Some(database_url(&mut args)?),
                    other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
                }
            }
            Ok(Command::Migrate { url })
        }
        Some(group @ ("checkpoint" | "key")) => {
            let mut rest = Vec::new();
            let mut url = None;
            while let Some(arg) = args.next() {
                if arg == "--database-url" {
                    url = Some(database_url(&mut args)?);
                } else {
                    rest.push(arg.clone());
                }
            }
            let command = chain::parse(group, &rest)?;
            Ok(Command::Chain { command, url })
        }
        Some(group @ ("client" | "credential")) => {
            let mut rest = Vec::new();
            let mut url = None;
            while let Some(arg) = args.next() {
                if arg == "--database-url" {
                    url = Some(database_url(&mut args)?);
                } else {
                    rest.push(arg.clone());
                }
            }
            let command = clients::parse(group, &rest)?;
            Ok(Command::Client { command, url })
        }
        Some("offline") => {
            let mut rest = Vec::new();
            let mut url = None;
            while let Some(arg) = args.next() {
                if arg == "--database-url" {
                    url = Some(database_url(&mut args)?);
                } else {
                    rest.push(arg.clone());
                }
            }
            let command = offline::parse(&rest)?;
            Ok(Command::Offline { command, url })
        }
        _ => Err(format!(
            "{USAGE}\n{}\n{}\n{}",
            clients::USAGE,
            chain::USAGE,
            offline::USAGE
        )),
    }
}

fn parse_verify(mut args: std::slice::Iter<'_, String>) -> Result<Command, String> {
    let mut command = VerifyCommand {
        options_scope: Scope::Incremental,
        settle: VerifyOptions::default().settle,
        record: true,
        chain: false,
        checkpoints: None,
        url: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--chain" => command.chain = true,
            "--checkpoints" => {
                command.checkpoints =
                    Some(args.next().ok_or("--checkpoints needs a directory")?.into());
            }
            "--full" => command.options_scope = Scope::Full,
            "--no-record" => command.record = false,
            "--settle-seconds" => {
                let seconds = args
                    .next()
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or("--settle-seconds needs a whole number of seconds")?;
                command.settle = Duration::from_secs(seconds);
            }
            "--database-url" => command.url = Some(database_url(&mut args)?),
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    if command.checkpoints.is_some() && !command.chain {
        return Err(format!("--checkpoints goes with --chain\n{USAGE}"));
    }
    Ok(Command::Verify(command))
}

/// The current time, from this machine's clock.
pub(crate) fn now() -> mezgeb_core::Timestamp {
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or_default();
    mezgeb_core::Timestamp::from_micros(i64::try_from(micros).unwrap_or(i64::MAX))
}

async fn connect(url: Option<String>) -> Result<PgConnection, String> {
    let url = url
        .or_else(|| std::env::var("MEZGEB_DATABASE_URL").ok())
        .ok_or("no database: pass --database-url or set MEZGEB_DATABASE_URL")?;
    PgConnection::connect(&url)
        .await
        .map_err(|e| format!("cannot connect: {e}"))
}

async fn run(command: Command) -> Result<bool, String> {
    match command {
        Command::Verify(command) if command.chain => {
            let mut conn = connect(command.url).await?;
            chain::verify(&mut conn, command.checkpoints.as_deref()).await
        }
        Command::Verify(command) => {
            let mut conn = connect(command.url).await?;
            let options = VerifyOptions {
                scope: command.options_scope,
                settle: command.settle,
                record: command.record,
            };
            let report = verify(&mut conn, &options)
                .await
                .map_err(|e| format!("verification failed to run: {e}"))?;
            print!("{report}");
            if let Some(run) = report.run_id {
                println!("recorded as verification run {run}");
            }
            Ok(report.is_clean())
        }
        Command::ClosePeriod {
            next,
            signing_key,
            url,
        } => {
            // Read the key before closing, so a bad key stops nothing half-way.
            let signer = signing_key
                .as_deref()
                .map(mezgeb_engine::Signer::from_file)
                .transpose()?;
            let mut conn = connect(url).await?;
            let report = close_period(&mut conn, &next)
                .await
                .map_err(|e| format!("the close failed to run: {e}"))?;
            let period = &report.period;
            let last = period.last_sequence.unwrap_or(period.after_sequence);
            let range = if last > period.after_sequence {
                format!("entries {}..={}", period.after_sequence + 1, last)
            } else {
                "no entries".to_owned()
            };
            println!(
                "period {} ({}): {}, {range}, {} accounts snapshotted",
                period.id, period.label, period.status, report.snapshots
            );
            if let Some(next) = &report.next {
                println!("postings now go to period {} ({})", next.id, next.label);
            }
            for finding in &report.findings {
                println!("finding: {finding}");
            }
            if let (true, Some(signer)) = (report.is_closed(), &signer) {
                let checkpoint =
                    mezgeb_engine::record_checkpoint(&mut conn, period.id, signer, now())
                        .await
                        .map_err(|e| format!("the checkpoint failed: {e}"))?;
                println!(
                    "checkpoint {} signed by key {}: entries to position {}, audit events to {}",
                    checkpoint.id,
                    checkpoint.key_id,
                    checkpoint.position,
                    checkpoint.audit_position
                );
                println!("export it with `mezgeb-cli checkpoint export` and keep a copy elsewhere");
            }
            Ok(report.is_closed())
        }
        Command::Migrate { url } => {
            let mut conn = connect(url).await?;
            MIGRATOR
                .run(&mut conn)
                .await
                .map_err(|e| format!("migration failed: {e}"))?;
            let latest = MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
            println!("schema at migration {latest}");
            Ok(true)
        }
        Command::Chain { command, url } => match command {
            chain::ChainCommand::Generate { out } => {
                print!("{}", chain::generate(&out)?);
                Ok(true)
            }
            chain::ChainCommand::Export { dir } => {
                let mut conn = connect(url).await?;
                let (clean, out) = chain::export(&mut conn, &dir).await?;
                print!("{out}");
                Ok(clean)
            }
        },
        Command::Client { command, url } => {
            let mut conn = connect(url).await?;
            print!("{}", clients::run(&mut conn, command).await?);
            Ok(true)
        }
        Command::Offline {
            command: offline::OfflineCommand::GenerateKey { out },
            ..
        } => {
            print!("{}", offline::generate(&out)?);
            Ok(true)
        }
        Command::Offline { command, url } => {
            let mut conn = connect(url).await?;
            let (out, clean) = offline::run(&mut conn, command).await?;
            print!("{out}");
            Ok(clean)
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse(&args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    match run(command).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_verify() {
        assert_eq!(
            parse(&args(&["verify"])),
            Ok(Command::Verify(VerifyCommand {
                options_scope: Scope::Incremental,
                settle: Duration::from_secs(60),
                record: true,
                chain: false,
                checkpoints: None,
                url: None,
            }))
        );
        assert_eq!(
            parse(&args(&[
                "verify",
                "--full",
                "--settle-seconds",
                "0",
                "--no-record",
                "--database-url",
                "postgres://x"
            ])),
            Ok(Command::Verify(VerifyCommand {
                options_scope: Scope::Full,
                settle: Duration::ZERO,
                record: false,
                chain: false,
                checkpoints: None,
                url: Some("postgres://x".into()),
            }))
        );
    }

    #[test]
    fn parses_close_period() {
        assert_eq!(
            parse(&args(&[
                "close-period",
                "--next",
                "2026-11-15",
                "--signing-key",
                "k.hex"
            ])),
            Ok(Command::ClosePeriod {
                next: "2026-11-15".into(),
                signing_key: Some("k.hex".into()),
                url: None,
            })
        );
        assert_eq!(
            parse(&args(&[
                "close-period",
                "--database-url",
                "postgres://x",
                "--next",
                "d2",
                "--no-checkpoint"
            ])),
            Ok(Command::ClosePeriod {
                next: "d2".into(),
                signing_key: None,
                url: Some("postgres://x".into()),
            })
        );
    }

    #[test]
    fn parses_verify_chain() {
        let Ok(Command::Verify(command)) =
            parse(&args(&["verify", "--chain", "--checkpoints", "copies"]))
        else {
            panic!("not a verify command");
        };
        assert!(command.chain);
        assert_eq!(command.checkpoints, Some("copies".into()));
        assert!(parse(&args(&["verify", "--checkpoints", "copies"])).is_err());
        assert!(matches!(
            parse(&args(&["checkpoint", "export", "--dir", "out"])),
            Ok(Command::Chain { .. })
        ));
    }

    #[test]
    fn parses_migrate() {
        assert_eq!(
            parse(&args(&["migrate"])),
            Ok(Command::Migrate { url: None })
        );
        assert_eq!(
            parse(&args(&["migrate", "--database-url", "postgres://x"])),
            Ok(Command::Migrate {
                url: Some("postgres://x".into())
            })
        );
    }

    #[test]
    fn refuses_bad_arguments() {
        assert!(parse(&args(&[])).is_err());
        assert!(parse(&args(&["import"])).is_err());
        assert!(parse(&args(&["verify", "--fast"])).is_err());
        assert!(parse(&args(&["verify", "--settle-seconds", "soon"])).is_err());
        assert!(parse(&args(&["verify", "--database-url"])).is_err());
        assert!(parse(&args(&["close-period"])).is_err());
        assert!(parse(&args(&["close-period", "--next"])).is_err());
        assert!(parse(&args(&["close-period", "--next", "d2", "--now"])).is_err());
        // Without a key a close would leave no checkpoint: refused unless asked.
        if std::env::var("MEZGEB_SIGNING_KEY_FILE").is_err() {
            assert!(parse(&args(&["close-period", "--next", "d2"])).is_err());
        }
        assert!(parse(&args(&["migrate", "--down"])).is_err());
    }
}
