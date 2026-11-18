//! Statement import, the external run, and the breaks (ADR-0020).

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use mezgeb_core::{AccountId, ClientId, EntryId, Timestamp};
use sqlx::postgres::PgRow;
use sqlx::{Connection, PgConnection, Row};
use uuid::Uuid;

use super::matching::{Coverage, Finding, Item, LedgerLine, MatchOptions, match_lines};
use super::statement::StatementLine;
use super::{
    BANK_REF_KEY, BreakKind, BreakStatus, PAYMENT_REF_KEY, ReconBreak, ReconError, ReconRun,
    RunKind, day_of,
};
use crate::rows::{account_id, storage};

/// Rows written per statement, to keep each parameter array bounded.
const CHUNK: usize = 20_000;

/// What an import did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub run_id: Uuid,
    /// Lines in the file.
    pub lines: usize,
    /// New lines stored.
    pub imported: usize,
    /// Lines already stored with the same content, or repeated in the file.
    pub skipped: usize,
    /// Lines whose ID was stored with other content (`statement_conflict`).
    pub conflicts: usize,
}

/// A break to record, before it has an ID.
#[derive(Debug, Clone, Default)]
pub(crate) struct NewBreak {
    pub kind: Option<BreakKind>,
    pub subject: String,
    pub account: Option<Uuid>,
    pub client: Option<Uuid>,
    pub item: Option<Uuid>,
    pub entry: Option<Uuid>,
    pub line_no: Option<i16>,
    pub device: Option<Uuid>,
    pub voucher: Option<Uuid>,
    pub other_minor: Option<i64>,
    pub ledger_minor: Option<i64>,
    pub difference_minor: Option<i64>,
    pub detail: String,
}

/// Serialises imports and runs of one scope, such as an account.
pub(crate) async fn lock_scope(tx: &mut PgConnection, scope: &str) -> Result<(), ReconError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("mezgeb.recon:{scope}"))
        .execute(&mut *tx)
        .await?;
    Ok(())
}

/// Imports statement lines for `account` (ADR-0020 decision 4). A line
/// already stored with the same content, or repeated in the file, is
/// skipped; the same line ID with other content is a `statement_conflict`
/// break, and the stored line stays as it is.
pub async fn import_statement(
    conn: &mut PgConnection,
    account: AccountId,
    lines: &[StatementLine],
    source: &str,
    now: Timestamp,
) -> Result<ImportReport, ReconError> {
    let started = Instant::now();
    if source.chars().count() > 300 {
        return Err(ReconError::Invalid(
            "the source is over 300 characters".into(),
        ));
    }
    let mut tx = conn.begin().await?;
    lock_scope(&mut tx, &account.to_string()).await?;
    let currency: Option<String> =
        sqlx::query_scalar("SELECT currency FROM accounts WHERE id = $1")
            .bind(account.as_uuid())
            .fetch_optional(&mut *tx)
            .await?;
    let currency = currency.ok_or_else(|| ReconError::NotFound(format!("account {account}")))?;
    if let Some(line) = lines.iter().find(|l| l.currency.code() != currency) {
        return Err(ReconError::Invalid(format!(
            "line {}: {} is not the account's currency {currency}",
            line.row,
            line.currency.code()
        )));
    }

    // The first of each line ID in the file; the rest repeat it or conflict.
    let mut first: HashMap<&str, (&StatementLine, [u8; 32])> = HashMap::new();
    let mut conflicts: Vec<(&StatementLine, [u8; 32])> = Vec::new();
    let mut skipped = 0;
    for line in lines {
        let digest = line.digest();
        match first.get(line.line_ref.as_str()) {
            None => {
                first.insert(&line.line_ref, (line, digest));
            }
            Some((_, seen)) if *seen == digest => skipped += 1,
            Some(_) => conflicts.push((line, digest)),
        }
    }
    let refs: Vec<String> = first.keys().map(|r| (*r).to_owned()).collect();
    let stored: HashMap<String, Vec<u8>> = sqlx::query(
        "SELECT line_ref, digest FROM recon_items WHERE account_id = $1 AND line_ref = ANY($2)",
    )
    .bind(account.as_uuid())
    .bind(&refs)
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| Ok((row.try_get("line_ref")?, row.try_get("digest")?)))
    .collect::<Result<_, sqlx::Error>>()?;

    let mut new: Vec<(&StatementLine, [u8; 32])> = Vec::new();
    // In file order, for a deterministic import.
    for line in lines {
        let Some((kept, digest)) = first.get(line.line_ref.as_str()) else {
            continue;
        };
        if !std::ptr::eq(*kept, line) {
            continue;
        }
        match stored.get(&line.line_ref) {
            None => new.push((line, *digest)),
            Some(old) if old[..] == digest[..] => skipped += 1,
            Some(_) => conflicts.push((line, *digest)),
        }
    }

    let run_id = Uuid::now_v7();
    for chunk in new.chunks(CHUNK) {
        let ids: Vec<Uuid> = chunk.iter().map(|_| Uuid::now_v7()).collect();
        let line_refs: Vec<&str> = chunk.iter().map(|(l, _)| l.line_ref.as_str()).collect();
        let dates: Vec<i32> = chunk.iter().map(|(l, _)| l.booking_date).collect();
        let amounts: Vec<i64> = chunk.iter().map(|(l, _)| l.amount_minor).collect();
        let references: Vec<&str> = chunk.iter().map(|(l, _)| l.reference.as_str()).collect();
        let digests: Vec<Vec<u8>> = chunk.iter().map(|(_, d)| d.to_vec()).collect();
        sqlx::query(
            "INSERT INTO recon_items
                 (id, import_run_id, account_id, line_ref, booking_date, amount_minor,
                  currency, reference, digest)
             SELECT u.id, $1, $2, u.line_ref, DATE '1970-01-01' + u.day, u.amount, $3,
                    u.reference, u.digest
               FROM unnest($4::uuid[], $5::text[], $6::int[], $7::bigint[], $8::text[],
                           $9::bytea[]) AS u (id, line_ref, day, amount, reference, digest)",
        )
        .bind(run_id)
        .bind(account.as_uuid())
        .bind(&currency)
        .bind(&ids)
        .bind(&line_refs)
        .bind(&dates)
        .bind(&amounts)
        .bind(&references)
        .bind(&digests)
        .execute(&mut *tx)
        .await?;
    }

    let breaks: Vec<NewBreak> = conflicts
        .iter()
        .map(|(line, digest)| NewBreak {
            kind: Some(BreakKind::StatementConflict),
            subject: format!("conflict:{account}:{}:{}", line.line_ref, hex(digest)),
            account: Some(account.as_uuid()),
            other_minor: Some(line.amount_minor),
            detail: format!(
                "line {}: the bank's line {} was imported before with other content",
                line.row, line.line_ref
            ),
            ..NewBreak::default()
        })
        .collect();
    let breaks_new = insert_breaks(&mut tx, run_id, &breaks, now).await?;
    let breaks_open = open_breaks_of_account(&mut tx, account).await?;
    insert_run(
        &mut tx,
        &RunRow {
            id: run_id,
            kind: RunKind::Import,
            account: Some(account.as_uuid()),
            client: None,
            source: source.to_owned(),
            as_of: now,
            items: new.len(),
            matched: 0,
            pending: 0,
            breaks_new,
            breaks_cleared: 0,
            breaks_open,
            started,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(ImportReport {
        run_id,
        lines: lines.len(),
        imported: new.len(),
        skipped,
        conflicts: conflicts.len(),
    })
}

/// Reconciles `account` against its imported statement lines (ADR-0020
/// decisions 5 and 6): matches what it can, records a break for what is
/// left and old enough, and clears the open breaks a match now explains.
pub async fn reconcile_account(
    conn: &mut PgConnection,
    account: AccountId,
    options: MatchOptions,
    now: Timestamp,
) -> Result<ReconRun, ReconError> {
    let started = Instant::now();
    if options.window_days < 0 || options.grace_days < 0 {
        return Err(ReconError::Invalid(
            "the window and grace must not be negative".into(),
        ));
    }
    let mut tx = conn.begin().await?;
    lock_scope(&mut tx, &account.to_string()).await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1)")
        .bind(account.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
    if !exists {
        return Err(ReconError::NotFound(format!("account {account}")));
    }
    let range = sqlx::query(
        "SELECT (min(booking_date) - DATE '1970-01-01')::int AS first,
                (max(booking_date) - DATE '1970-01-01')::int AS last
           FROM recon_items WHERE account_id = $1",
    )
    .bind(account.as_uuid())
    .fetch_one(&mut *tx)
    .await?;
    let coverage = match (
        range.try_get::<Option<i32>, _>("first")?,
        range.try_get::<Option<i32>, _>("last")?,
    ) {
        (Some(first), Some(last)) => Some(Coverage { first, last }),
        _ => None,
    };

    // Unmatched statement lines, except those held by an open mismatch.
    let items: Vec<Item> = sqlx::query(
        "SELECT i.id, i.line_ref, (i.booking_date - DATE '1970-01-01')::int AS day,
                i.amount_minor, i.reference
           FROM recon_items i
          WHERE i.account_id = $1 AND i.match_rule IS NULL
            AND NOT EXISTS (SELECT 1 FROM recon_breaks b
                             WHERE b.item_id = i.id AND b.status = 'open'
                               AND b.kind = 'amount_mismatch')",
    )
    .bind(account.as_uuid())
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| {
        Ok(Item {
            id: row.try_get("id")?,
            line_ref: row.try_get("line_ref")?,
            date: row.try_get("day")?,
            amount: row.try_get("amount_minor")?,
            reference: row.try_get("reference")?,
        })
    })
    .collect::<Result<_, sqlx::Error>>()?;

    // Keys of matched lines on the dates of unmatched ones, to tell a
    // repeat from a missing line.
    let dates: Vec<i32> = items
        .iter()
        .map(|i| i.date)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let matched_keys = sqlx::query(
        "SELECT (booking_date - DATE '1970-01-01')::int AS day, amount_minor, reference
           FROM recon_items
          WHERE account_id = $1 AND match_rule IS NOT NULL
            AND booking_date = ANY(SELECT DATE '1970-01-01' + d FROM unnest($2::int[]) AS d)",
    )
    .bind(account.as_uuid())
    .bind(&dates)
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| {
        Ok((
            row.try_get("day")?,
            row.try_get("amount_minor")?,
            row.try_get("reference")?,
        ))
    })
    .collect::<Result<HashSet<_>, sqlx::Error>>()?;

    // Ledger lines not yet accounted for, from just before the statements
    // begin. A line is accounted for once a statement line matched it, or a
    // resolved break names it, or an open mismatch holds it.
    let since = coverage.map_or(i32::MAX, |c| c.first.saturating_sub(options.window_days));
    let lines: Vec<LedgerLine> = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT l.entry_id, l.line_no, e.sequence,
                floor(extract(epoch FROM coalesce(e.event_time, e.recorded_time)) / 86400)::int
                    AS day,
                CASE l.side WHEN 'debit' THEN l.amount ELSE -l.amount END AS signed,
                e.metadata ->> '{BANK_REF_KEY}' AS bank_ref,
                e.metadata ->> '{PAYMENT_REF_KEY}' AS payment_ref
           FROM journal_lines l
           JOIN journal_entries e ON e.id = l.entry_id
          WHERE l.account_id = $1
            AND coalesce(e.event_time, e.recorded_time)
                    >= timestamptz 'epoch' + $2::bigint * interval '1 day'
            AND NOT EXISTS (SELECT 1 FROM recon_items i
                             WHERE i.matched_entry_id = l.entry_id
                               AND i.matched_line_no = l.line_no)
            AND NOT EXISTS (SELECT 1 FROM recon_breaks b
                             WHERE b.entry_id = l.entry_id AND b.line_no = l.line_no
                               AND (b.status = 'resolved'
                                    OR (b.status = 'open' AND b.kind = 'amount_mismatch')))
            AND NOT EXISTS (SELECT 1 FROM recon_breaks b
                             WHERE b.resolution_entry_id = l.entry_id
                               AND b.resolution_line_no = l.line_no)"
    )))
    .bind(account.as_uuid())
    .bind(i64::from(since))
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| {
        Ok(LedgerLine {
            entry: row.try_get("entry_id")?,
            line_no: row.try_get("line_no")?,
            sequence: row.try_get("sequence")?,
            date: row.try_get("day")?,
            amount: row.try_get("signed")?,
            bank_ref: row.try_get("bank_ref")?,
            payment_ref: row.try_get("payment_ref")?,
        })
    })
    .collect::<Result<_, sqlx::Error>>()?;

    let result = match_lines(
        &items,
        &lines,
        &matched_keys,
        coverage,
        day_of(now),
        options,
    );

    let run_id = Uuid::now_v7();
    for chunk in result.matches.chunks(CHUNK) {
        let ids: Vec<Uuid> = chunk.iter().map(|(i, ..)| items[*i].id).collect();
        let entries: Vec<Uuid> = chunk.iter().map(|(_, l, _)| lines[*l].entry).collect();
        let line_nos: Vec<i16> = chunk.iter().map(|(_, l, _)| lines[*l].line_no).collect();
        let rules: Vec<&str> = chunk.iter().map(|(.., r)| r.as_str()).collect();
        sqlx::query(
            "UPDATE recon_items i
                SET matched_entry_id = u.entry_id, matched_line_no = u.line_no,
                    match_rule = u.rule, matched_run_id = $1
               FROM unnest($2::uuid[], $3::uuid[], $4::smallint[], $5::text[])
                    AS u (id, entry_id, line_no, rule)
              WHERE i.id = u.id",
        )
        .bind(run_id)
        .bind(&ids)
        .bind(&entries)
        .bind(&line_nos)
        .bind(&rules)
        .execute(&mut *tx)
        .await?;
    }

    let account_uuid = Some(account.as_uuid());
    let breaks: Vec<NewBreak> = result
        .findings
        .iter()
        .map(|finding| match *finding {
            Finding::MissingInLedger { item } | Finding::Duplicate { item } => {
                let it = &items[item];
                let duplicate = matches!(finding, Finding::Duplicate { .. });
                NewBreak {
                    kind: Some(if duplicate {
                        BreakKind::DuplicateStatementLine
                    } else {
                        BreakKind::MissingInLedger
                    }),
                    subject: format!("item:{}", it.id),
                    account: account_uuid,
                    item: Some(it.id),
                    other_minor: Some(it.amount),
                    ledger_minor: Some(0),
                    difference_minor: Some(it.amount),
                    detail: if duplicate {
                        format!(
                            "the bank's line {} repeats another line's date, amount and reference",
                            it.line_ref
                        )
                    } else {
                        format!("the bank's line {} has no ledger line", it.line_ref)
                    },
                    ..NewBreak::default()
                }
            }
            Finding::MissingInStatement { line } => {
                let l = &lines[line];
                NewBreak {
                    kind: Some(BreakKind::MissingInStatement),
                    subject: format!("line:{}:{}", l.entry, l.line_no),
                    account: account_uuid,
                    entry: Some(l.entry),
                    line_no: Some(l.line_no),
                    other_minor: Some(0),
                    ledger_minor: Some(l.amount),
                    difference_minor: Some(-l.amount),
                    detail: "no statement line for this ledger line".into(),
                    ..NewBreak::default()
                }
            }
            Finding::AmountMismatch { item, line } => {
                let (it, l) = (&items[item], &lines[line]);
                NewBreak {
                    kind: Some(BreakKind::AmountMismatch),
                    subject: format!("item:{}", it.id),
                    account: account_uuid,
                    item: Some(it.id),
                    entry: Some(l.entry),
                    line_no: Some(l.line_no),
                    other_minor: Some(it.amount),
                    ledger_minor: Some(l.amount),
                    difference_minor: Some(it.amount - l.amount),
                    detail: format!(
                        "the bank's line {} and the ledger line with its bank_ref differ",
                        it.line_ref
                    ),
                    ..NewBreak::default()
                }
            }
        })
        .collect();
    let breaks_new = insert_breaks(&mut tx, run_id, &breaks, now).await?;

    // A break about a line that is now matched is cleared.
    let breaks_cleared = sqlx::query(
        "UPDATE recon_breaks b
            SET status = 'cleared', cleared_by_run_id = $2,
                closed_at = timestamptz 'epoch' + $3::bigint * interval '1 microsecond'
          WHERE b.status = 'open' AND b.account_id = $1
            AND b.kind IN ('missing_in_ledger', 'duplicate_statement_line', 'missing_in_statement')
            AND (EXISTS (SELECT 1 FROM recon_items i
                          WHERE i.id = b.item_id AND i.match_rule IS NOT NULL)
                 OR (b.kind = 'missing_in_statement'
                     AND EXISTS (SELECT 1 FROM recon_items i
                                  WHERE i.matched_entry_id = b.entry_id
                                    AND i.matched_line_no = b.line_no)))",
    )
    .bind(account.as_uuid())
    .bind(run_id)
    .bind(now.as_micros())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let breaks_open = open_breaks_of_account(&mut tx, account).await?;

    let run = RunRow {
        id: run_id,
        kind: RunKind::External,
        account: account_uuid,
        client: None,
        source: String::new(),
        as_of: now,
        items: items.len() + lines.len(),
        matched: result.matches.len(),
        pending: result.pending_items.len() + result.pending_lines.len(),
        breaks_new,
        breaks_cleared: usize::try_from(breaks_cleared).map_err(storage)?,
        breaks_open,
        started,
    };
    insert_run(&mut tx, &run).await?;
    let recorded = get_run(&mut tx, run_id).await?;
    tx.commit().await?;
    Ok(recorded)
}

/// Resolves a money break with the entry that put the reconciled account
/// right (ADR-0020 decision 7). The entry must have been booked by one of
/// `templates`, must not have resolved another break, and its one line in
/// the account must equal the break's difference, in amount and direction.
pub async fn resolve_break(
    conn: &mut PgConnection,
    id: Uuid,
    entry: EntryId,
    by: ClientId,
    templates: &[&str],
    now: Timestamp,
) -> Result<ReconBreak, ReconError> {
    let mut tx = conn.begin().await?;
    let found = break_for_update(&mut tx, id).await?;
    if found.status != BreakStatus::Open {
        return Err(ReconError::Closed(found.status));
    }
    if !found.kind.is_money() {
        return Err(ReconError::Conflict(format!(
            "a {} break is explained, not resolved by an entry",
            found.kind
        )));
    }
    let (Some(account), Some(difference)) = (found.account, found.difference_minor) else {
        return Err(ReconError::Conflict(
            "the break has no account difference".into(),
        ));
    };
    let template: Option<Option<String>> =
        sqlx::query_scalar("SELECT metadata ->> 'template' FROM journal_entries WHERE id = $1")
            .bind(entry.as_uuid())
            .fetch_optional(&mut *tx)
            .await?;
    let template = template.ok_or_else(|| ReconError::NotFound(format!("entry {entry}")))?;
    if !template.as_deref().is_some_and(|t| templates.contains(&t)) {
        return Err(ReconError::Conflict(format!(
            "the entry was not booked by a reconciliation template ({})",
            templates.join(", ")
        )));
    }
    let lines: Vec<(i16, i64)> = sqlx::query(
        "SELECT line_no, CASE side WHEN 'debit' THEN amount ELSE -amount END AS signed
           FROM journal_lines WHERE entry_id = $1 AND account_id = $2",
    )
    .bind(entry.as_uuid())
    .bind(account.as_uuid())
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| Ok((row.try_get("line_no")?, row.try_get("signed")?)))
    .collect::<Result<_, sqlx::Error>>()?;
    let [(line_no, signed)] = lines[..] else {
        return Err(ReconError::Conflict(
            "the entry must have exactly one line in the reconciled account".into(),
        ));
    };
    if signed != difference {
        return Err(ReconError::Conflict(format!(
            "the entry moves {signed} in the account; the break's difference is {difference}"
        )));
    }
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM recon_items
                         WHERE matched_entry_id = $1 AND matched_line_no = $2)
             OR EXISTS (SELECT 1 FROM recon_breaks
                         WHERE resolution_entry_id = $1
                            OR (entry_id = $1 AND line_no = $2 AND status = 'resolved'))",
    )
    .bind(entry.as_uuid())
    .bind(line_no)
    .fetch_one(&mut *tx)
    .await?;
    if taken {
        return Err(ReconError::Conflict(
            "the entry is already matched or has resolved a break".into(),
        ));
    }
    // A run between the posting and this call may have reported the
    // resolving line itself as missing from the statement: it is accounted
    // for now.
    sqlx::query(
        "UPDATE recon_breaks
            SET status = 'explained', resolution_note = $3, closed_by = $4,
                closed_at = timestamptz 'epoch' + $5::bigint * interval '1 microsecond'
          WHERE entry_id = $1 AND line_no = $2 AND status = 'open'",
    )
    .bind(entry.as_uuid())
    .bind(line_no)
    .bind(format!("the line resolves break {id}"))
    .bind(by.as_uuid())
    .bind(now.as_micros())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE recon_breaks
            SET status = 'resolved', resolution_entry_id = $2, resolution_line_no = $3,
                closed_by = $4,
                closed_at = timestamptz 'epoch' + $5::bigint * interval '1 microsecond'
          WHERE id = $1",
    )
    .bind(id)
    .bind(entry.as_uuid())
    .bind(line_no)
    .bind(by.as_uuid())
    .bind(now.as_micros())
    .execute(&mut *tx)
    .await?;
    // The statement line is accounted for: by the ledger line it disagreed
    // with (a mismatch), or by the resolving line (money the ledger lacked).
    if let Some(item) = found.item {
        let (matched_entry, matched_line) = match (found.kind, found.line) {
            (BreakKind::AmountMismatch, Some(line)) => line,
            _ => (entry.as_uuid(), line_no),
        };
        sqlx::query(
            "UPDATE recon_items
                SET matched_entry_id = $2, matched_line_no = $3, match_rule = 'resolution'
              WHERE id = $1 AND match_rule IS NULL",
        )
        .bind(item)
        .bind(matched_entry)
        .bind(matched_line)
        .execute(&mut *tx)
        .await?;
    }
    let resolved = break_for_update(&mut tx, id).await?;
    tx.commit().await?;
    Ok(resolved)
}

/// Closes a break that is not about money with an operator's reason
/// (ADR-0020 decision 8).
pub async fn explain_break(
    conn: &mut PgConnection,
    id: Uuid,
    note: &str,
    by: ClientId,
    now: Timestamp,
) -> Result<ReconBreak, ReconError> {
    let note = note.trim();
    if note.is_empty() || note.chars().count() > 1000 {
        return Err(ReconError::Invalid(
            "the reason must be 1 to 1,000 characters".into(),
        ));
    }
    let mut tx = conn.begin().await?;
    let found = break_for_update(&mut tx, id).await?;
    if found.status != BreakStatus::Open {
        return Err(ReconError::Closed(found.status));
    }
    if found.kind.is_money() {
        return Err(ReconError::Conflict(format!(
            "a {} break is resolved by a suspense entry, not explained",
            found.kind
        )));
    }
    sqlx::query(
        "UPDATE recon_breaks
            SET status = 'explained', resolution_note = $2, closed_by = $3,
                closed_at = timestamptz 'epoch' + $4::bigint * interval '1 microsecond'
          WHERE id = $1",
    )
    .bind(id)
    .bind(note)
    .bind(by.as_uuid())
    .bind(now.as_micros())
    .execute(&mut *tx)
    .await?;
    let explained = break_for_update(&mut tx, id).await?;
    tx.commit().await?;
    Ok(explained)
}

/// Which breaks to list, oldest first, after `after` (a break ID) if given.
#[derive(Debug, Clone, Default)]
pub struct BreakFilter {
    pub status: Option<BreakStatus>,
    pub kind: Option<BreakKind>,
    pub account: Option<AccountId>,
    pub client: Option<ClientId>,
    pub after: Option<Uuid>,
    /// 1 to 1,000.
    pub limit: i64,
}

/// Breaks matching `filter`, in ID order (oldest first).
pub async fn list_breaks(
    conn: &mut PgConnection,
    filter: &BreakFilter,
) -> Result<Vec<ReconBreak>, ReconError> {
    if !(1..=1000).contains(&filter.limit) {
        return Err(ReconError::Invalid("the limit must be 1 to 1,000".into()));
    }
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {BREAK_COLUMNS} FROM recon_breaks
          WHERE ($1::text IS NULL OR status = $1)
            AND ($2::text IS NULL OR kind = $2)
            AND ($3::uuid IS NULL OR account_id = $3)
            AND ($4::uuid IS NULL OR client_id = $4)
            AND ($5::uuid IS NULL OR id > $5)
          ORDER BY id LIMIT $6"
    )))
    .bind(filter.status.map(BreakStatus::as_str))
    .bind(filter.kind.map(BreakKind::as_str))
    .bind(filter.account.map(AccountId::as_uuid))
    .bind(filter.client.map(ClientId::as_uuid))
    .bind(filter.after)
    .bind(filter.limit)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter().map(break_from_row).collect()
}

/// One break.
pub async fn get_break(conn: &mut PgConnection, id: Uuid) -> Result<ReconBreak, ReconError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {BREAK_COLUMNS} FROM recon_breaks WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ReconError::NotFound(format!("break {id}")))?;
    break_from_row(&row)
}

/// The latest runs, newest first, of one kind if given.
pub async fn list_runs(
    conn: &mut PgConnection,
    kind: Option<RunKind>,
    limit: i64,
) -> Result<Vec<ReconRun>, ReconError> {
    if !(1..=1000).contains(&limit) {
        return Err(ReconError::Invalid("the limit must be 1 to 1,000".into()));
    }
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {RUN_COLUMNS} FROM recon_runs
          WHERE ($1::text IS NULL OR kind = $1)
          ORDER BY recorded_at DESC, id DESC LIMIT $2"
    )))
    .bind(kind.map(RunKind::as_str))
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter().map(run_from_row).collect()
}

async fn break_for_update(tx: &mut PgConnection, id: Uuid) -> Result<ReconBreak, ReconError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {BREAK_COLUMNS} FROM recon_breaks WHERE id = $1 FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ReconError::NotFound(format!("break {id}")))?;
    break_from_row(&row)
}

/// Records breaks found by `run`, skipping any whose subject already has an
/// open break. Returns how many were new.
pub(crate) async fn insert_breaks(
    tx: &mut PgConnection,
    run: Uuid,
    breaks: &[NewBreak],
    now: Timestamp,
) -> Result<usize, ReconError> {
    let mut inserted = 0;
    for chunk in breaks.chunks(CHUNK) {
        let ids: Vec<Uuid> = chunk.iter().map(|_| Uuid::now_v7()).collect();
        let kinds: Vec<&str> = chunk
            .iter()
            .map(|b| b.kind.map_or("", BreakKind::as_str))
            .collect();
        let subjects: Vec<&str> = chunk.iter().map(|b| b.subject.as_str()).collect();
        let accounts: Vec<Option<Uuid>> = chunk.iter().map(|b| b.account).collect();
        let clients: Vec<Option<Uuid>> = chunk.iter().map(|b| b.client).collect();
        let items: Vec<Option<Uuid>> = chunk.iter().map(|b| b.item).collect();
        let entries: Vec<Option<Uuid>> = chunk.iter().map(|b| b.entry).collect();
        let line_nos: Vec<Option<i16>> = chunk.iter().map(|b| b.line_no).collect();
        let devices: Vec<Option<Uuid>> = chunk.iter().map(|b| b.device).collect();
        let vouchers: Vec<Option<Uuid>> = chunk.iter().map(|b| b.voucher).collect();
        let others: Vec<Option<i64>> = chunk.iter().map(|b| b.other_minor).collect();
        let ledgers: Vec<Option<i64>> = chunk.iter().map(|b| b.ledger_minor).collect();
        let differences: Vec<Option<i64>> = chunk.iter().map(|b| b.difference_minor).collect();
        let details: Vec<&str> = chunk.iter().map(|b| b.detail.as_str()).collect();
        let result = sqlx::query(
            "INSERT INTO recon_breaks
                 (id, run_id, kind, subject, account_id, client_id, item_id, entry_id, line_no,
                  device_id, voucher_id, other_minor, ledger_minor, difference_minor, detail,
                  found_at)
             SELECT u.id, $1, u.kind, u.subject, u.account, u.client, u.item, u.entry, u.line_no,
                    u.device, u.voucher, u.other, u.ledger, u.difference, u.detail,
                    timestamptz 'epoch' + $2::bigint * interval '1 microsecond'
               FROM unnest($3::uuid[], $4::text[], $5::text[], $6::uuid[], $7::uuid[],
                           $8::uuid[], $9::uuid[], $10::smallint[], $11::uuid[], $12::uuid[],
                           $13::bigint[], $14::bigint[], $15::bigint[], $16::text[])
                    AS u (id, kind, subject, account, client, item, entry, line_no, device,
                          voucher, other, ledger, difference, detail)
             ON CONFLICT (subject) WHERE status = 'open' DO NOTHING",
        )
        .bind(run)
        .bind(now.as_micros())
        .bind(&ids)
        .bind(&kinds)
        .bind(&subjects)
        .bind(&accounts)
        .bind(&clients)
        .bind(&items)
        .bind(&entries)
        .bind(&line_nos)
        .bind(&devices)
        .bind(&vouchers)
        .bind(&others)
        .bind(&ledgers)
        .bind(&differences)
        .bind(&details)
        .execute(&mut *tx)
        .await?;
        inserted += usize::try_from(result.rows_affected()).map_err(storage)?;
    }
    Ok(inserted)
}

async fn open_breaks_of_account(
    tx: &mut PgConnection,
    account: AccountId,
) -> Result<usize, ReconError> {
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recon_breaks WHERE account_id = $1 AND status = 'open'",
    )
    .bind(account.as_uuid())
    .fetch_one(&mut *tx)
    .await?;
    Ok(usize::try_from(open).map_err(storage)?)
}

/// A run to record once its counts are known.
pub(crate) struct RunRow {
    pub id: Uuid,
    pub kind: RunKind,
    pub account: Option<Uuid>,
    pub client: Option<Uuid>,
    pub source: String,
    pub as_of: Timestamp,
    pub items: usize,
    pub matched: usize,
    pub pending: usize,
    pub breaks_new: usize,
    pub breaks_cleared: usize,
    pub breaks_open: usize,
    pub started: Instant,
}

pub(crate) async fn insert_run(tx: &mut PgConnection, run: &RunRow) -> Result<(), ReconError> {
    let count = |n: usize| i32::try_from(n).map_err(storage);
    let runtime = i64::try_from(run.started.elapsed().as_micros()).map_err(storage)?;
    sqlx::query(
        "INSERT INTO recon_runs
             (id, kind, account_id, client_id, source, as_of, items, matched, pending,
              breaks_new, breaks_cleared, breaks_open, runtime_micros)
         VALUES ($1, $2, $3, $4, $5, timestamptz 'epoch' + $6::bigint * interval '1 microsecond',
                 $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(run.id)
    .bind(run.kind.as_str())
    .bind(run.account)
    .bind(run.client)
    .bind(&run.source)
    .bind(run.as_of.as_micros())
    .bind(count(run.items)?)
    .bind(count(run.matched)?)
    .bind(count(run.pending)?)
    .bind(count(run.breaks_new)?)
    .bind(count(run.breaks_cleared)?)
    .bind(count(run.breaks_open)?)
    .bind(runtime)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

pub(crate) async fn get_run(conn: &mut PgConnection, id: Uuid) -> Result<ReconRun, ReconError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {RUN_COLUMNS} FROM recon_runs WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ReconError::NotFound(format!("run {id}")))?;
    run_from_row(&row)
}

const RUN_COLUMNS: &str = "id, kind, account_id, client_id, source, \
     (extract(epoch FROM as_of) * 1000000)::bigint AS as_of_micros, items, matched, pending, \
     breaks_new, breaks_cleared, breaks_open, runtime_micros, \
     (extract(epoch FROM recorded_at) * 1000000)::bigint AS recorded_at_micros";

const BREAK_COLUMNS: &str = "id, run_id, kind, subject, account_id, client_id, item_id, \
     entry_id, line_no, device_id, voucher_id, other_minor, ledger_minor, difference_minor, \
     detail, (extract(epoch FROM found_at) * 1000000)::bigint AS found_at_micros, status, \
     cleared_by_run_id, resolution_entry_id, resolution_line_no, resolution_note, closed_by, \
     (extract(epoch FROM closed_at) * 1000000)::bigint AS closed_at_micros, \
     coalesce((SELECT a.currency FROM accounts a WHERE a.id = recon_breaks.account_id), \
              (SELECT v.currency FROM vouchers v WHERE v.id = recon_breaks.voucher_id)) \
         AS currency";

fn client(uuid: Option<Uuid>) -> Result<Option<ClientId>, ReconError> {
    uuid.map(|u| ClientId::from_uuid(u).map_err(|e| ReconError::Ledger(storage(e))))
        .transpose()
}

fn run_from_row(row: &PgRow) -> Result<ReconRun, ReconError> {
    let kind: String = row.try_get("kind")?;
    let account: Option<Uuid> = row.try_get("account_id")?;
    Ok(ReconRun {
        id: row.try_get("id")?,
        kind: kind
            .parse()
            .map_err(|e: String| ReconError::Ledger(storage(e)))?,
        account: account.map(account_id).transpose()?,
        client: client(row.try_get("client_id")?)?,
        source: row.try_get("source")?,
        as_of: Timestamp::from_micros(row.try_get("as_of_micros")?),
        items: row.try_get("items")?,
        matched: row.try_get("matched")?,
        pending: row.try_get("pending")?,
        breaks_new: row.try_get("breaks_new")?,
        breaks_cleared: row.try_get("breaks_cleared")?,
        breaks_open: row.try_get("breaks_open")?,
        runtime_micros: row.try_get("runtime_micros")?,
        recorded_at: Timestamp::from_micros(row.try_get("recorded_at_micros")?),
    })
}

fn break_from_row(row: &PgRow) -> Result<ReconBreak, ReconError> {
    let kind: String = row.try_get("kind")?;
    let status: String = row.try_get("status")?;
    let account: Option<Uuid> = row.try_get("account_id")?;
    let entry: Option<Uuid> = row.try_get("entry_id")?;
    let line_no: Option<i16> = row.try_get("line_no")?;
    let resolution: Option<Uuid> = row.try_get("resolution_entry_id")?;
    let resolution_line: Option<i16> = row.try_get("resolution_line_no")?;
    let closed: Option<i64> = row.try_get("closed_at_micros")?;
    let bad = |e: String| ReconError::Ledger(storage(e));
    Ok(ReconBreak {
        id: row.try_get("id")?,
        run_id: row.try_get("run_id")?,
        kind: kind.parse().map_err(bad)?,
        subject: row.try_get("subject")?,
        account: account.map(account_id).transpose()?,
        client: client(row.try_get("client_id")?)?,
        item: row.try_get("item_id")?,
        line: entry.zip(line_no),
        device: row.try_get("device_id")?,
        voucher: row.try_get("voucher_id")?,
        other_minor: row.try_get("other_minor")?,
        ledger_minor: row.try_get("ledger_minor")?,
        difference_minor: row.try_get("difference_minor")?,
        currency: row
            .try_get::<Option<String>, _>("currency")?
            .map(|c| mezgeb_core::Currency::from_code(&c).map_err(|e| bad(e.to_string())))
            .transpose()?,
        detail: row.try_get("detail")?,
        found_at: Timestamp::from_micros(row.try_get("found_at_micros")?),
        status: status.parse().map_err(bad)?,
        cleared_by_run: row.try_get("cleared_by_run_id")?,
        resolution: resolution.zip(resolution_line),
        resolution_note: row.try_get("resolution_note")?,
        closed_by: client(row.try_get("closed_by")?)?,
        closed_at: closed.map(Timestamp::from_micros),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
