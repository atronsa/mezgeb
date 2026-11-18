//! `mezgeb-cli recon`: statement import and the daily reconciliation (step
//! 2.15, ADR-0020, `docs/runbooks/reconciliation.md`).
//!
//! ```text
//! mezgeb-cli recon import ACCOUNT FILE [--mapping FILE]
//! mezgeb-cli recon run ACCOUNT [--window-days N] [--grace-days N]
//! mezgeb-cli recon offline
//! mezgeb-cli recon breaks [--status S] [--kind K] [--limit N]
//! ```
//!
//! - `import` reads a bank statement for an account (a chart reference such
//!   as `1010`), in Mezgeb's CSV layout or the bank's, described by a
//!   mapping file (`docs/reconciliation/statement-format.md`). A line
//!   already imported is skipped; the same line ID with other content is a
//!   `statement_conflict` break. Exit status 1 if there was a conflict.
//! - `run` reconciles the account against its statement lines. Exit status
//!   1 if any break is open for the account afterwards.
//! - `offline` reconciles devices and escrows. Exit status 1 if any
//!   offline break is open.
//! - `breaks` lists breaks, oldest first (open ones unless `--status`).
//!
//! Connect as `mezgeb_verifier`: it reads the ledger and writes only the
//! reconciliation tables. Resolving and explaining breaks is done through
//! the API, by an operator, so that it is authorised and audited.

use std::fmt::Write as _;
use std::path::PathBuf;

use mezgeb_core::AccountId;
use mezgeb_engine::account_by_reference;
use mezgeb_engine::recon::{
    BreakFilter, BreakKind, BreakStatus, Mapping, MatchOptions, ReconBreak, ReconRun,
    import_statement, list_breaks, parse_statement, reconcile_account, reconcile_offline,
};
use sqlx::PgConnection;

use crate::now;

pub const USAGE: &str = "       mezgeb-cli recon import ACCOUNT FILE [--mapping FILE] \
     [--database-url URL]\n       \
     mezgeb-cli recon run ACCOUNT [--window-days N] [--grace-days N] [--database-url URL]\n       \
     mezgeb-cli recon offline [--database-url URL]\n       \
     mezgeb-cli recon breaks [--status open|cleared|resolved|explained] [--kind KIND] \
     [--limit N] [--database-url URL]";

/// A parsed `recon` command.
#[derive(Debug, PartialEq, Eq)]
pub enum ReconCommand {
    Import {
        account: String,
        file: PathBuf,
        mapping: Option<PathBuf>,
    },
    Run {
        account: String,
        options: MatchOptions,
    },
    Offline,
    Breaks {
        status: BreakStatus,
        kind: Option<BreakKind>,
        limit: i64,
    },
}

/// Parses the words after `recon`. `--database-url` is taken out by the
/// caller.
pub fn parse(args: &[String]) -> Result<ReconCommand, String> {
    let usage = || format!("usage:\n{USAGE}");
    let mut words = Vec::new();
    let mut mapping = None;
    let mut options = MatchOptions::default();
    let mut status = BreakStatus::Open;
    let mut kind = None;
    let mut limit = 100;
    let mut args = args.iter();
    let days = |value: Option<&String>, flag: &str| -> Result<i32, String> {
        value
            .and_then(|v| v.parse::<i32>().ok())
            .filter(|d| (0..=60).contains(d))
            .ok_or(format!("{flag} needs a number of days from 0 to 60"))
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mapping" => mapping = Some(PathBuf::from(args.next().ok_or_else(usage)?)),
            "--window-days" => options.window_days = days(args.next(), arg)?,
            "--grace-days" => options.grace_days = days(args.next(), arg)?,
            "--status" => status = args.next().ok_or_else(usage)?.parse()?,
            "--kind" => kind = Some(args.next().ok_or_else(usage)?.parse()?),
            "--limit" => {
                limit = args
                    .next()
                    .and_then(|l| l.parse::<i64>().ok())
                    .filter(|l| (1..=1000).contains(l))
                    .ok_or("--limit needs a number from 1 to 1,000")?;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option {other:?}\n{}", usage()));
            }
            word => words.push(word.to_owned()),
        }
    }
    match words.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["import", account, file] => Ok(ReconCommand::Import {
            account: account.to_owned(),
            file: PathBuf::from(file),
            mapping,
        }),
        ["run", account] => Ok(ReconCommand::Run {
            account: account.to_owned(),
            options,
        }),
        ["offline"] => Ok(ReconCommand::Offline),
        ["breaks"] => Ok(ReconCommand::Breaks {
            status,
            kind,
            limit,
        }),
        _ => Err(usage()),
    }
}

async fn account(
    conn: &mut PgConnection,
    reference: &str,
) -> Result<(AccountId, mezgeb_core::Currency), String> {
    let account = account_by_reference(conn, reference)
        .await
        .map_err(|e| e.to_string())?
        .ok_or(format!("no account {reference}"))?;
    Ok((account.id(), account.currency()))
}

fn summary(run: &ReconRun) -> String {
    format!(
        "run {} ({}): {} items, {} matched, {} pending; breaks: {} new, {} cleared, {} open; \
         {:.1} s\n",
        run.id,
        run.kind,
        run.items,
        run.matched,
        run.pending,
        run.breaks_new,
        run.breaks_cleared,
        run.breaks_open,
        run.runtime_micros as f64 / 1e6
    )
}

fn describe(b: &ReconBreak) -> String {
    let money = match b.difference_minor {
        Some(d) => {
            let sign = if d < 0 { "-" } else { "" };
            let d = d.unsigned_abs();
            format!(" difference {sign}{}.{:02}", d / 100, d % 100)
        }
        None => String::new(),
    };
    format!("{} {} {}{money}: {}\n", b.id, b.status, b.kind, b.detail)
}

/// Runs a command; returns its output and whether it is clean.
pub async fn run(conn: &mut PgConnection, command: ReconCommand) -> Result<(String, bool), String> {
    let mut out = String::new();
    match command {
        ReconCommand::Import {
            account: reference,
            file,
            mapping,
        } => {
            let mapping = match mapping {
                Some(path) => Mapping::from_toml(
                    &std::fs::read_to_string(&path)
                        .map_err(|e| format!("cannot read {}: {e}", path.display()))?,
                )
                .map_err(|e| format!("{}: {e}", path.display()))?,
                None => Mapping::mezgeb(),
            };
            let text = std::fs::read_to_string(&file)
                .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let (account, currency) = account(conn, &reference).await?;
            let lines = parse_statement(&text, &mapping, currency)
                .map_err(|e| format!("{}: {e}", file.display()))?;
            let source = format!("{} ({})", file.display(), mapping.name);
            let report = import_statement(conn, account, &lines, &source, now())
                .await
                .map_err(|e| e.to_string())?;
            writeln!(
                out,
                "imported {} of {} lines into {reference} ({} already imported, {} conflicts); \
                 run {}",
                report.imported, report.lines, report.skipped, report.conflicts, report.run_id
            )
            .map_err(|e| e.to_string())?;
            Ok((out, report.conflicts == 0))
        }
        ReconCommand::Run {
            account: reference,
            options,
        } => {
            let (account, _) = account(conn, &reference).await?;
            let run = reconcile_account(conn, account, options, now())
                .await
                .map_err(|e| e.to_string())?;
            out.push_str(&summary(&run));
            Ok((out, run.breaks_open == 0))
        }
        ReconCommand::Offline => {
            let run = reconcile_offline(conn, now())
                .await
                .map_err(|e| e.to_string())?;
            out.push_str(&summary(&run));
            Ok((out, run.breaks_open == 0))
        }
        ReconCommand::Breaks {
            status,
            kind,
            limit,
        } => {
            let breaks = list_breaks(
                conn,
                &BreakFilter {
                    status: Some(status),
                    kind,
                    limit,
                    ..BreakFilter::default()
                },
            )
            .await
            .map_err(|e| e.to_string())?;
            for b in &breaks {
                out.push_str(&describe(b));
            }
            writeln!(out, "{} {status} breaks", breaks.len()).map_err(|e| e.to_string())?;
            Ok((out, true))
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
    fn parses_every_command() {
        assert_eq!(
            parse(&args(&[
                "import",
                "1010",
                "nov.csv",
                "--mapping",
                "bank.toml"
            ])),
            Ok(ReconCommand::Import {
                account: "1010".into(),
                file: "nov.csv".into(),
                mapping: Some("bank.toml".into()),
            })
        );
        assert_eq!(
            parse(&args(&[
                "run",
                "1010",
                "--grace-days",
                "0",
                "--window-days",
                "5"
            ])),
            Ok(ReconCommand::Run {
                account: "1010".into(),
                options: MatchOptions {
                    window_days: 5,
                    grace_days: 0,
                },
            })
        );
        assert_eq!(parse(&args(&["offline"])), Ok(ReconCommand::Offline));
        assert_eq!(
            parse(&args(&["breaks", "--kind", "stale_cache", "--limit", "5"])),
            Ok(ReconCommand::Breaks {
                status: BreakStatus::Open,
                kind: Some(BreakKind::StaleCache),
                limit: 5,
            })
        );
        for bad in [
            &["run"][..],
            &["run", "1010", "--grace-days", "-1"],
            &["breaks", "--kind", "nope"],
            &["breaks", "--limit", "0"],
            &["import", "1010"],
            &["run", "1010", "--fast"],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_break_is_one_line() {
        let text = describe(&ReconBreak {
            id: uuid::Uuid::nil(),
            run_id: uuid::Uuid::nil(),
            kind: BreakKind::AmountMismatch,
            subject: "item:x".into(),
            account: None,
            client: None,
            item: None,
            line: None,
            device: None,
            voucher: None,
            other_minor: Some(19_000),
            ledger_minor: Some(20_000),
            difference_minor: Some(-50),
            currency: None,
            detail: "differ".into(),
            found_at: mezgeb_core::Timestamp::from_micros(0),
            status: BreakStatus::Open,
            cleared_by_run: None,
            resolution: None,
            resolution_note: None,
            closed_by: None,
            closed_at: None,
        });
        assert_eq!(
            text,
            "00000000-0000-0000-0000-000000000000 open amount_mismatch difference -0.50: differ\n"
        );
    }
}
