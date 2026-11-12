//! Balance verification (step 1.13, ADR-0012).
//!
//! A stored balance is trustworthy only because it is continuously checked
//! against the journal. [`verify`] runs every check in one consistent
//! snapshot and reports each problem with the exact account and entry:
//!
//! 1. each entry balances per currency (INV-1);
//! 2. each entry has at least two lines, and no line is orphaned (INV-4);
//! 3. each line's stored totals continue the account's chain (INV-6);
//! 4. each account's totals equal its checkpoint plus its lines (INV-5);
//! 5. all debits equal all credits per currency (INV-2).
//!
//! Every run is recorded in `verification_runs`. A clean run also stores
//! each account's totals at its checkpoint, so an incremental run only
//! checks entries posted since the last clean run (checks 1–3), while
//! checks 4 and 5 always cover every account.
//!
//! **Which entries a run covers.** A run checkpoints only entries recorded at
//! least `settle` ago. Posting transactions are limited to 30 seconds
//! (`transaction_timeout` on `mezgeb_app`), so with the default 60 seconds,
//! every entry below the checkpoint is either committed or will never
//! appear: no entry can be skipped by committing late.

use std::fmt;
use std::time::{Duration, Instant};

use sqlx::{Acquire, PgConnection, Row};
use uuid::Uuid;

/// Full or incremental verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every entry, from the beginning.
    Full,
    /// Only entries after the last clean run's checkpoint (a full run if
    /// there is none). Account totals and the global balance are still
    /// checked for every account.
    Incremental,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Scope::Full => "full",
            Scope::Incremental => "incremental",
        }
    }
}

/// How to run a verification.
#[derive(Debug, Clone)]
pub struct VerifyOptions {
    pub scope: Scope,
    /// Only entries recorded at least this long ago are checkpointed (see
    /// the module documentation). Tests with no concurrent postings use 0.
    pub settle: Duration,
    /// Record the run in `verification_runs` (and its checkpoint, if clean).
    pub record: bool,
}

impl Default for VerifyOptions {
    fn default() -> VerifyOptions {
        VerifyOptions {
            scope: Scope::Incremental,
            settle: Duration::from_secs(60),
            record: true,
        }
    }
}

/// One problem found, with exactly where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// INV-1: debits and credits of an entry differ in a currency.
    EntryUnbalanced {
        entry: Uuid,
        currency: String,
        debits: i64,
        credits: i64,
    },
    /// INV-1, INV-4: an entry has fewer than two lines.
    EntryTooFewLines { entry: Uuid, lines: i64 },
    /// INV-4: a line whose entry does not exist.
    OrphanLine { entry: Uuid, line_no: i16 },
    /// INV-6: the first line of an account whose stored totals do not
    /// continue the account's chain.
    ChainBroken {
        account: Uuid,
        entry: Uuid,
        line_no: i16,
        stored: (i64, i64),
        expected: (i64, i64),
    },
    /// INV-5: an account's stored totals differ from its lines.
    AccountTotals {
        account: Uuid,
        stored: (i64, i64),
        expected: (i64, i64),
    },
    /// INV-2: all debits differ from all credits in a currency.
    GlobalUnbalanced {
        currency: String,
        debits: i64,
        credits: i64,
    },
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Finding::EntryUnbalanced {
                entry,
                currency,
                debits,
                credits,
            } => write!(
                f,
                "INV-1 entry {entry}: {currency} debits {debits} ≠ credits {credits}"
            ),
            Finding::EntryTooFewLines { entry, lines } => {
                write!(
                    f,
                    "INV-4 entry {entry}: {lines} line(s), at least 2 required"
                )
            }
            Finding::OrphanLine { entry, line_no } => {
                write!(
                    f,
                    "INV-4 line {line_no} of entry {entry}: the entry does not exist"
                )
            }
            Finding::ChainBroken {
                account,
                entry,
                line_no,
                stored,
                expected,
            } => write!(
                f,
                "INV-6 account {account}: line {line_no} of entry {entry} stores totals \
                 {stored:?}, the chain gives {expected:?}"
            ),
            Finding::AccountTotals {
                account,
                stored,
                expected,
            } => write!(
                f,
                "INV-5 account {account}: stores totals {stored:?}, its lines give {expected:?}"
            ),
            Finding::GlobalUnbalanced {
                currency,
                debits,
                credits,
            } => write!(
                f,
                "INV-2 {currency}: all debits {debits} ≠ all credits {credits}"
            ),
        }
    }
}

/// The result of one verification run.
#[derive(Debug, Clone)]
pub struct VerificationReport {
    /// The recorded run, if `record` was set.
    pub run_id: Option<i64>,
    pub scope: Scope,
    /// Entries with `from_sequence < sequence <= to_sequence` were checked.
    pub from_sequence: i64,
    pub to_sequence: i64,
    pub lines_checked: i64,
    pub duration: Duration,
    pub findings: Vec<Finding>,
}

impl VerificationReport {
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}

impl fmt::Display for VerificationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} verification of entries {}..={}: {} line(s) in {} ms: {}",
            self.scope.as_str(),
            self.from_sequence + 1,
            self.to_sequence,
            self.lines_checked,
            self.duration.as_millis(),
            if self.is_clean() { "clean" } else { "DIRTY" }
        )?;
        for finding in &self.findings {
            writeln!(f, "  {finding}")?;
        }
        Ok(())
    }
}

/// At most this many findings of each kind are reported.
const LIMIT: i64 = 100;

/// Runs every check in one consistent snapshot, and records the run.
///
/// Connect as `mezgeb_verifier`, which can read the ledger and record runs
/// but post nothing.
pub async fn verify(
    conn: &mut PgConnection,
    options: &VerifyOptions,
) -> Result<VerificationReport, sqlx::Error> {
    let started = Instant::now();
    let mut tx = conn.begin().await?;
    // Every query below sees the same snapshot of the ledger.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    let started_at: sqlx::types::time::OffsetDateTime = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *tx)
        .await?;

    // The last clean run, for an incremental check.
    let previous: Option<(i64, i64)> = match options.scope {
        Scope::Full => None,
        Scope::Incremental => {
            sqlx::query_as(
                "SELECT id, to_sequence FROM verification_runs
                  WHERE result = 'clean' ORDER BY id DESC LIMIT 1",
            )
            .fetch_optional(&mut *tx)
            .await?
        }
    };
    let (checkpoint_run, from): (Option<i64>, i64) = match previous {
        Some((run, to)) => (Some(run), to),
        None => (None, 0),
    };
    let to: i64 = sqlx::query_scalar(
        "SELECT coalesce(max(sequence), $1) FROM journal_entries
          WHERE sequence > $1
            AND recorded_time <= clock_timestamp() - $2::bigint * interval '1 millisecond'",
    )
    .bind(from)
    .bind(i64::try_from(options.settle.as_millis()).unwrap_or(i64::MAX))
    .fetch_one(&mut *tx)
    .await?;

    // The lines in scope, gathered once: every check below reads this table
    // instead of joining a million lines to their entries again. The sorts
    // and hashes get enough memory to stay off disk.
    sqlx::query("SET LOCAL work_mem = '256MB'")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "CREATE TEMP TABLE verify_scope ON COMMIT DROP AS
         SELECT l.account_id, l.entry_id, l.line_no, e.sequence, l.currency,
                CASE WHEN l.side = 'debit' THEN l.amount ELSE 0 END AS debit,
                CASE WHEN l.side = 'credit' THEN l.amount ELSE 0 END AS credit,
                l.debits_after, l.credits_after
           FROM journal_lines l JOIN journal_entries e ON e.id = l.entry_id
          WHERE e.sequence > $1 AND e.sequence <= $2",
    )
    .bind(from)
    .bind(to)
    .execute(&mut *tx)
    .await?;
    sqlx::query("ANALYZE verify_scope")
        .execute(&mut *tx)
        .await?;

    let mut findings = Vec::new();

    // 1. Entries that do not balance.
    let rows = sqlx::query(
        "SELECT entry_id, currency, sum(debit)::bigint AS debits, sum(credit)::bigint AS credits
           FROM verify_scope
          GROUP BY entry_id, currency
         HAVING sum(debit) <> sum(credit)
          ORDER BY entry_id LIMIT $1",
    )
    .bind(LIMIT)
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        findings.push(Finding::EntryUnbalanced {
            entry: row.try_get("entry_id")?,
            currency: row.try_get("currency")?,
            debits: row.try_get("debits")?,
            credits: row.try_get("credits")?,
        });
    }

    // 2. Entries with fewer than two lines, and lines without an entry.
    let rows = sqlx::query(
        "SELECT e.id, count(s.entry_id) AS lines
           FROM journal_entries e LEFT JOIN verify_scope s ON s.entry_id = e.id
          WHERE e.sequence > $1 AND e.sequence <= $2
          GROUP BY e.id HAVING count(s.entry_id) < 2
          ORDER BY e.id LIMIT $3",
    )
    .bind(from)
    .bind(to)
    .bind(LIMIT)
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        findings.push(Finding::EntryTooFewLines {
            entry: row.try_get("id")?,
            lines: row.try_get("lines")?,
        });
    }
    let rows = sqlx::query(
        "SELECT l.entry_id, l.line_no FROM journal_lines l
          WHERE NOT EXISTS (SELECT 1 FROM journal_entries e WHERE e.id = l.entry_id)
          ORDER BY l.entry_id, l.line_no LIMIT $1",
    )
    .bind(LIMIT)
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        findings.push(Finding::OrphanLine {
            entry: row.try_get("entry_id")?,
            line_no: row.try_get("line_no")?,
        });
    }

    // 3. The chain of totals after each line, per account, starting from
    //    the checkpoint; only the first break per account is reported.
    let rows = sqlx::query(
        "WITH checkpoint AS (
             SELECT account_id, debits_total, credits_total
               FROM verification_checkpoints WHERE run_id = $1
         ),
         chained AS (
             SELECT s.*,
                    coalesce(c.debits_total, 0)
                      + sum(s.debit) OVER running AS expected_debits,
                    coalesce(c.credits_total, 0)
                      + sum(s.credit) OVER running AS expected_credits
               FROM verify_scope s LEFT JOIN checkpoint c USING (account_id)
             WINDOW running AS (PARTITION BY s.account_id ORDER BY s.sequence, s.line_no
                                ROWS UNBOUNDED PRECEDING)
         )
         SELECT DISTINCT ON (account_id)
                account_id, entry_id, line_no, debits_after, credits_after,
                expected_debits::bigint AS expected_debits,
                expected_credits::bigint AS expected_credits
           FROM chained
          WHERE debits_after <> expected_debits OR credits_after <> expected_credits
          ORDER BY account_id, sequence, line_no
          LIMIT $2",
    )
    .bind(checkpoint_run)
    .bind(LIMIT)
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        findings.push(Finding::ChainBroken {
            account: row.try_get("account_id")?,
            entry: row.try_get("entry_id")?,
            line_no: row.try_get("line_no")?,
            stored: (row.try_get("debits_after")?, row.try_get("credits_after")?),
            expected: (
                row.try_get("expected_debits")?,
                row.try_get("expected_credits")?,
            ),
        });
    }

    // 4. Every account's totals: its checkpoint plus every line after it.
    let rows = sqlx::query(
        "WITH checkpoint AS (
             SELECT account_id, debits_total, credits_total
               FROM verification_checkpoints WHERE run_id = $2
         ),
         recent AS (
             SELECT account_id, sum(debit) AS debits, sum(credit) AS credits
               FROM (
                   SELECT account_id, debit, credit FROM verify_scope
                   UNION ALL
                   SELECT l.account_id,
                          CASE WHEN l.side = 'debit' THEN l.amount ELSE 0 END,
                          CASE WHEN l.side = 'credit' THEN l.amount ELSE 0 END
                     FROM journal_lines l JOIN journal_entries e ON e.id = l.entry_id
                    WHERE e.sequence > $1
               ) lines
              GROUP BY account_id
         ),
         expected AS (
             SELECT a.id, a.debits_total, a.credits_total,
                    (coalesce(c.debits_total, 0) + coalesce(r.debits, 0))::bigint AS expected_debits,
                    (coalesce(c.credits_total, 0) + coalesce(r.credits, 0))::bigint AS expected_credits
               FROM accounts a
               LEFT JOIN checkpoint c ON c.account_id = a.id
               LEFT JOIN recent r ON r.account_id = a.id
         )
         SELECT * FROM expected
          WHERE debits_total <> expected_debits OR credits_total <> expected_credits
          ORDER BY id LIMIT $3",
    )
    .bind(to)
    .bind(checkpoint_run)
    .bind(LIMIT)
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        findings.push(Finding::AccountTotals {
            account: row.try_get("id")?,
            stored: (row.try_get("debits_total")?, row.try_get("credits_total")?),
            expected: (
                row.try_get("expected_debits")?,
                row.try_get("expected_credits")?,
            ),
        });
    }

    // 5. All debits equal all credits, per currency.
    let rows = sqlx::query(
        "SELECT currency, sum(debits_total)::bigint AS debits, sum(credits_total)::bigint AS credits
           FROM accounts GROUP BY currency
         HAVING sum(debits_total) <> sum(credits_total)
          ORDER BY currency",
    )
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        findings.push(Finding::GlobalUnbalanced {
            currency: row.try_get("currency")?,
            debits: row.try_get("debits")?,
            credits: row.try_get("credits")?,
        });
    }

    let lines_checked: i64 = sqlx::query_scalar("SELECT count(*) FROM verify_scope")
        .fetch_one(&mut *tx)
        .await?;

    let duration = started.elapsed();
    let mut report = VerificationReport {
        run_id: None,
        scope: options.scope,
        from_sequence: from,
        to_sequence: to,
        lines_checked,
        duration,
        findings,
    };

    if options.record {
        let findings: Vec<String> = report.findings.iter().map(ToString::to_string).collect();
        let run_id: i64 = sqlx::query_scalar(
            "INSERT INTO verification_runs
                 (scope, from_sequence, to_sequence, started_at, finished_at, duration_ms,
                  lines_checked, result, findings)
             VALUES ($1, $2, $3, $4, clock_timestamp(), $5, $6, $7, $8::jsonb)
             RETURNING id",
        )
        .bind(options.scope.as_str())
        .bind(from)
        .bind(to)
        .bind(started_at)
        .bind(i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .bind(lines_checked)
        .bind(if report.is_clean() { "clean" } else { "dirty" })
        .bind(serde_json::to_string(&findings).unwrap_or_else(|_| "[]".into()))
        .fetch_one(&mut *tx)
        .await?;
        if report.is_clean() {
            // The new checkpoint: the previous one plus the lines just verified.
            sqlx::query(
                "INSERT INTO verification_checkpoints
                     (run_id, account_id, debits_total, credits_total)
                 SELECT $1, account_id, sum(debits)::bigint, sum(credits)::bigint
                   FROM (
                       SELECT account_id, debits_total AS debits, credits_total AS credits
                         FROM verification_checkpoints WHERE run_id = $2
                       UNION ALL
                       SELECT account_id, debit, credit FROM verify_scope
                   ) totals
                  GROUP BY account_id",
            )
            .bind(run_id)
            .bind(checkpoint_run)
            .execute(&mut *tx)
            .await?;
        }
        report.run_id = Some(run_id);
    }
    tx.commit().await?;
    Ok(report)
}
