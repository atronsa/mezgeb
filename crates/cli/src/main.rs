//! Operator command line.
//!
//! ```text
//! mezgeb-cli verify [--full] [--settle-seconds N] [--no-record] [--database-url URL]
//! mezgeb-cli close-period --next LABEL [--database-url URL]
//! ```
//!
//! `verify` proves that the stored balances equal the journal
//! (docs/db-schema.md#verification). Connect as `mezgeb_verifier`.
//!
//! `close-period` closes the open accounting period and opens the next one,
//! named `LABEL`; if a close stopped part-way, it finishes that one instead
//! (docs/runbooks/close-period.md). Connect as `mezgeb_app`.
//!
//! The URL comes from `--database-url` or `MEZGEB_DATABASE_URL`.
//!
//! Exit status: 0 clean (or closed), 1 problems found (the period stays
//! closing), 2 could not run.
//!
//! Planned: `verify --chain` (step 2.09) and `import` (step 3.14).

use std::process::ExitCode;
use std::time::Duration;

use mezgeb_engine::{Scope, VerifyOptions, close_period, verify};
use sqlx::{Connection, PgConnection};

const USAGE: &str = "usage: mezgeb-cli verify [--full] [--settle-seconds N] [--no-record] \
                     [--database-url URL]\n       \
                     mezgeb-cli close-period --next LABEL [--database-url URL]";

/// The parsed `verify` command.
#[derive(Debug, PartialEq, Eq)]
struct VerifyCommand {
    options_scope: Scope,
    settle: Duration,
    record: bool,
    url: Option<String>,
}

/// A parsed command.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Verify(VerifyCommand),
    ClosePeriod { next: String, url: Option<String> },
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
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--next" => next = Some(args.next().ok_or("--next needs a label")?.clone()),
                    "--database-url" => url = Some(database_url(&mut args)?),
                    other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
                }
            }
            let next = next.ok_or(format!("close-period needs --next LABEL\n{USAGE}"))?;
            Ok(Command::ClosePeriod { next, url })
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn parse_verify(mut args: std::slice::Iter<'_, String>) -> Result<Command, String> {
    let mut command = VerifyCommand {
        options_scope: Scope::Incremental,
        settle: VerifyOptions::default().settle,
        record: true,
        url: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
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
    Ok(Command::Verify(command))
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
        Command::ClosePeriod { next, url } => {
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
            Ok(report.is_closed())
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
                url: Some("postgres://x".into()),
            }))
        );
    }

    #[test]
    fn parses_close_period() {
        assert_eq!(
            parse(&args(&["close-period", "--next", "2026-11-15"])),
            Ok(Command::ClosePeriod {
                next: "2026-11-15".into(),
                url: None,
            })
        );
        assert_eq!(
            parse(&args(&[
                "close-period",
                "--database-url",
                "postgres://x",
                "--next",
                "d2"
            ])),
            Ok(Command::ClosePeriod {
                next: "d2".into(),
                url: Some("postgres://x".into()),
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
    }
}
