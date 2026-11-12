//! Operator command line.
//!
//! ```text
//! mezgeb-cli verify [--full] [--settle-seconds N] [--no-record] [--database-url URL]
//! ```
//!
//! `verify` proves that the stored balances equal the journal
//! (docs/db-schema.md#verification). Connect as `mezgeb_verifier`. The URL
//! comes from `--database-url` or `MEZGEB_DATABASE_URL`.
//!
//! Exit status: 0 clean, 1 problems found, 2 could not run.
//!
//! Planned: `verify --chain` (step 2.09) and `import` (step 3.14).

use std::process::ExitCode;
use std::time::Duration;

use mezgeb_engine::{Scope, VerifyOptions, verify};
use sqlx::{Connection, PgConnection};

const USAGE: &str = "usage: mezgeb-cli verify [--full] [--settle-seconds N] [--no-record] \
                     [--database-url URL]";

/// The parsed `verify` command.
#[derive(Debug, PartialEq, Eq)]
struct VerifyCommand {
    options_scope: Scope,
    settle: Duration,
    record: bool,
    url: Option<String>,
}

fn parse(args: &[String]) -> Result<VerifyCommand, String> {
    let mut args = args.iter();
    match args.next().map(String::as_str) {
        Some("verify") => {}
        _ => return Err(USAGE.to_owned()),
    }
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
            "--database-url" => {
                command.url = Some(args.next().ok_or("--database-url needs a URL")?.clone());
            }
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    Ok(command)
}

async fn run(command: VerifyCommand) -> Result<bool, String> {
    let url = command
        .url
        .or_else(|| std::env::var("MEZGEB_DATABASE_URL").ok())
        .ok_or("no database: pass --database-url or set MEZGEB_DATABASE_URL")?;
    let mut conn = PgConnection::connect(&url)
        .await
        .map_err(|e| format!("cannot connect: {e}"))?;
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
            Ok(VerifyCommand {
                options_scope: Scope::Incremental,
                settle: Duration::from_secs(60),
                record: true,
                url: None,
            })
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
            Ok(VerifyCommand {
                options_scope: Scope::Full,
                settle: Duration::ZERO,
                record: false,
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
    }
}
