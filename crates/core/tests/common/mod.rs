//! Shared helpers for tests that read the accounting documentation.

#![allow(dead_code)]

use mezgeb_core::{AccountType, Currency, Money, Side};

pub const CHART: &str = include_str!("../../../../charts/wallet-provider.toml");
pub const CHART_DOC: &str = include_str!("../../../../docs/accounting/chart-of-accounts.md");

/// The opening position and worked entries E01–E18, by file.
pub const ENTRY_DOCS: [(&str, &str); 6] = [
    (
        "README.md",
        include_str!("../../../../docs/accounting/entries/README.md"),
    ),
    (
        "deposits-and-withdrawals.md",
        include_str!("../../../../docs/accounting/entries/deposits-and-withdrawals.md"),
    ),
    (
        "payments.md",
        include_str!("../../../../docs/accounting/entries/payments.md"),
    ),
    (
        "refunds-and-corrections.md",
        include_str!("../../../../docs/accounting/entries/refunds-and-corrections.md"),
    ),
    (
        "settlement.md",
        include_str!("../../../../docs/accounting/entries/settlement.md"),
    ),
    (
        "offline-escrow.md",
        include_str!("../../../../docs/accounting/entries/offline-escrow.md"),
    ),
];

/// The cells of a Markdown table row, trimmed, without the outer pipes.
pub fn cells(line: &str) -> Option<Vec<&str>> {
    let inner = line.trim().strip_prefix('|')?.strip_suffix('|')?;
    Some(inner.split('|').map(str::trim).collect())
}

pub fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

pub fn parse_type(name: &str) -> Option<AccountType> {
    match name {
        "Asset" => Some(AccountType::Asset),
        "Liability" => Some(AccountType::Liability),
        "Equity" => Some(AccountType::Equity),
        "Revenue" => Some(AccountType::Revenue),
        "Expense" => Some(AccountType::Expense),
        _ => None,
    }
}

/// An ETB amount as written in the docs: `1,000`, `39.20`, `**107.80**`.
pub fn parse_etb(text: &str) -> Option<Money> {
    let plain: String = text.chars().filter(|c| !matches!(c, ',' | '*')).collect();
    Money::parse(&plain, Currency::ETB).ok()
}

/// One line of a worked entry, as written in the docs.
#[derive(Debug, Clone)]
pub struct FixtureLine {
    pub reference: String,
    pub account_type: AccountType,
    pub side: Side,
    pub amount: Money,
}

/// One journal table from the docs, such as `E13 #2` (the reversal).
#[derive(Debug, Clone)]
pub struct Fixture {
    /// The section heading the table is under, plus its number within it.
    pub label: String,
    pub lines: Vec<FixtureLine>,
    /// The amount on the table's **Total** row.
    pub total: Money,
}

/// Every journal table in the entry docs, in document order. Blank template
/// rows (no account) are skipped.
pub fn fixtures() -> Result<Vec<Fixture>, String> {
    let mut fixtures = Vec::new();
    for (file, text) in ENTRY_DOCS {
        let mut heading = String::from(file);
        let mut table_in_heading = 0;
        let mut lines: Vec<FixtureLine> = Vec::new();
        for raw in text.lines() {
            if let Some(title) = raw.strip_prefix("## ") {
                heading = title.split('.').next().unwrap_or(title).to_owned();
                table_in_heading = 0;
            }
            let Some(row) = cells(raw) else { continue };
            let number = row.first().copied().unwrap_or_default();
            let account = row.get(1).copied().unwrap_or_default();
            if is_digits(number) && !account.is_empty() {
                let reference = account.split_whitespace().next().unwrap_or_default();
                let type_name = row.get(2).copied().unwrap_or_default();
                let account_type = parse_type(type_name)
                    .ok_or_else(|| format!("{file}: unknown type {type_name:?}"))?;
                let (side, amount_text) = match (row.get(3), row.get(4)) {
                    (Some(dr), _) if !dr.is_empty() => (Side::Debit, *dr),
                    (_, Some(cr)) if !cr.is_empty() => (Side::Credit, *cr),
                    _ => return Err(format!("{file}: {reference} has no amount")),
                };
                let amount = parse_etb(amount_text)
                    .ok_or_else(|| format!("{file}: bad amount {amount_text:?}"))?;
                lines.push(FixtureLine {
                    reference: reference.to_owned(),
                    account_type,
                    side,
                    amount,
                });
            } else if account.contains("Total") && !lines.is_empty() {
                table_in_heading += 1;
                let total_text = row.get(3).copied().unwrap_or_default();
                let total = parse_etb(total_text)
                    .ok_or_else(|| format!("{file}: bad total {total_text:?}"))?;
                fixtures.push(Fixture {
                    label: format!("{heading} #{table_in_heading}"),
                    lines: std::mem::take(&mut lines),
                    total,
                });
            }
        }
    }
    Ok(fixtures)
}

pub const TRIAL_BALANCE: &str = include_str!("../../../../docs/accounting/trial-balance.md");

/// The rows of the trial balance after E18: each account reference and its
/// balance on its normal side, as printed in `docs/accounting/trial-balance.md`.
pub fn trial_balance() -> Result<Vec<(String, Money)>, String> {
    let start = TRIAL_BALANCE
        .find("## Trial balance after E18")
        .ok_or("no trial balance section")?;
    let section = &TRIAL_BALANCE[start..];
    let section = &section[..section.find("## Checks").unwrap_or(section.len())];
    let mut rows = Vec::new();
    for row in section.lines().filter_map(cells) {
        let reference = row.first().copied().unwrap_or_default();
        let code = reference.split(':').next().unwrap_or_default();
        if code.len() != 4 || !is_digits(code) {
            continue;
        }
        let amount = |i: usize| -> Result<i64, String> {
            match row.get(i).copied().unwrap_or_default() {
                "" => Ok(0),
                text => parse_etb(text)
                    .map(Money::amount)
                    .ok_or_else(|| format!("bad amount {text:?} for {reference}")),
            }
        };
        let (dr, cr) = (amount(4)?, amount(5)?);
        let balance = match row.get(3).copied() {
            Some("Dr") => dr - cr,
            Some("Cr") => cr - dr,
            other => return Err(format!("bad normal side {other:?} for {reference}")),
        };
        rows.push((
            reference.to_owned(),
            Money::from_minor(balance, Currency::ETB),
        ));
    }
    Ok(rows)
}
