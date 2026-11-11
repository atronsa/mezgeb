//! Checks `charts/wallet-provider.toml` against the accounting documentation
//! (step 1.05): the chart file and `docs/accounting/chart-of-accounts.md`
//! agree, and every line of every worked entry posts to an account that
//! exists in the chart, with the type the entry says.

use std::collections::BTreeSet;

use mezgeb_core::{AccountKind, AccountType, Chart, ChartStatus, Side};

const CHART: &str = include_str!("../../../charts/wallet-provider.toml");
const CHART_DOC: &str = include_str!("../../../docs/accounting/chart-of-accounts.md");
const ENTRY_DOCS: [(&str, &str); 6] = [
    (
        "README.md (opening position)",
        include_str!("../../../docs/accounting/entries/README.md"),
    ),
    (
        "deposits-and-withdrawals.md",
        include_str!("../../../docs/accounting/entries/deposits-and-withdrawals.md"),
    ),
    (
        "payments.md",
        include_str!("../../../docs/accounting/entries/payments.md"),
    ),
    (
        "refunds-and-corrections.md",
        include_str!("../../../docs/accounting/entries/refunds-and-corrections.md"),
    ),
    (
        "settlement.md",
        include_str!("../../../docs/accounting/entries/settlement.md"),
    ),
    (
        "offline-escrow.md",
        include_str!("../../../docs/accounting/entries/offline-escrow.md"),
    ),
];

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The cells of a Markdown table row, trimmed, without the outer pipes.
fn cells(line: &str) -> Option<Vec<&str>> {
    let inner = line.trim().strip_prefix('|')?.strip_suffix('|')?;
    Some(inner.split('|').map(str::trim).collect())
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn parse_type(name: &str) -> Option<AccountType> {
    match name {
        "Asset" => Some(AccountType::Asset),
        "Liability" => Some(AccountType::Liability),
        "Equity" => Some(AccountType::Equity),
        "Revenue" => Some(AccountType::Revenue),
        "Expense" => Some(AccountType::Expense),
        _ => None,
    }
}

#[test]
fn chart_file_loads() -> TestResult {
    let chart = Chart::from_toml(CHART)?;
    assert_eq!(chart.currency().code(), "ETB");
    assert_eq!(chart.accounts().len(), 15);
    let active = chart
        .accounts()
        .iter()
        .filter(|a| a.status() == ChartStatus::Active)
        .count();
    assert_eq!(active, 10);
    Ok(())
}

#[test]
fn chart_file_matches_the_documentation() -> TestResult {
    let chart = Chart::from_toml(CHART)?;
    let mut documented = BTreeSet::new();

    // Account rows start with a four-digit code: | 1010 | Name | ... |
    for row in CHART_DOC.lines().filter_map(cells) {
        let Some(code_text) = row.first() else {
            continue;
        };
        if code_text.len() != 4 || !is_digits(code_text) {
            continue;
        }
        let code: u16 = code_text.parse()?;
        let account = chart
            .accounts()
            .iter()
            .find(|a| a.code().value() == code)
            .ok_or_else(|| format!("{code} is documented but not in the chart file"))?;

        assert_eq!(row.get(1).copied(), Some(account.name()), "name of {code}");

        let kind = if row.contains(&"Control") {
            AccountKind::Control
        } else if row.contains(&"System") {
            AccountKind::System
        } else {
            return Err(format!("no kind for {code}").into());
        };
        assert_eq!(kind, account.kind(), "kind of {code}");

        let side = if row.contains(&"Dr") {
            Side::Debit
        } else if row.contains(&"Cr") {
            Side::Credit
        } else {
            return Err(format!("no normal side for {code}").into());
        };
        assert_eq!(side, account.normal_side(), "normal side of {code}");

        let status = if row.iter().any(|c| c.starts_with("Active")) {
            ChartStatus::Active
        } else if row.iter().any(|c| c.starts_with("Reserved")) {
            ChartStatus::Reserved
        } else {
            return Err(format!("no status for {code}").into());
        };
        assert_eq!(status, account.status(), "status of {code}");

        documented.insert(code);
    }

    let in_file: BTreeSet<u16> = chart.accounts().iter().map(|a| a.code().value()).collect();
    assert_eq!(documented, in_file, "documented codes and chart file codes");
    Ok(())
}

#[test]
fn every_worked_entry_line_uses_a_chart_account() -> TestResult {
    let chart = Chart::from_toml(CHART)?;
    let mut used = BTreeSet::new();
    let mut lines = 0;

    // Entry lines look like: | 1 | 2010:cust-abebe Customer wallet | Liability | 300 | |
    for (file, text) in ENTRY_DOCS {
        for row in text.lines().filter_map(cells) {
            let (Some(number), Some(account), Some(type_name)) =
                (row.first(), row.get(1), row.get(2))
            else {
                continue;
            };
            // Skip the blank rows of the entry template in README.md.
            if !is_digits(number) || account.is_empty() {
                continue;
            }
            let reference = account.split_whitespace().next().unwrap_or_default();
            let (template, _) = chart
                .resolve(reference)
                .map_err(|e| format!("{file}: {reference}: {e}"))?;
            let expected = parse_type(type_name)
                .ok_or_else(|| format!("{file}: unknown type {type_name:?}"))?;
            assert_eq!(
                template.account_type(),
                expected,
                "{file}: {reference} is booked as {type_name}"
            );
            used.insert(template.code().value());
            lines += 1;
        }
    }

    // 24 entry tables with two or three lines each.
    assert!(lines >= 48, "only {lines} entry lines found");

    // Every active account is used by some entry; reserved ones cannot be,
    // because `resolve` rejects them.
    let active: BTreeSet<u16> = chart
        .accounts()
        .iter()
        .filter(|a| a.status() == ChartStatus::Active)
        .map(|a| a.code().value())
        .collect();
    assert_eq!(used, active, "accounts used by the entries");
    Ok(())
}
