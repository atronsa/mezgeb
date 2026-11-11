//! Checks `charts/wallet-provider.toml` against the accounting documentation
//! (step 1.05): the chart file and `docs/accounting/chart-of-accounts.md`
//! agree, and every line of every worked entry posts to an account that
//! exists in the chart, with the type the entry says.

mod common;

use std::collections::BTreeSet;

use common::{CHART, CHART_DOC, cells, fixtures, is_digits};
use mezgeb_core::{AccountKind, Chart, ChartStatus, Side};

type TestResult = Result<(), Box<dyn std::error::Error>>;

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

    for fixture in fixtures()? {
        for line in &fixture.lines {
            let (template, _) = chart
                .resolve(&line.reference)
                .map_err(|e| format!("{}: {}: {e}", fixture.label, line.reference))?;
            assert_eq!(
                template.account_type(),
                line.account_type,
                "{}: {} has the wrong type",
                fixture.label,
                line.reference
            );
            used.insert(template.code().value());
        }
    }

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
