//! Matching statement lines with ledger lines (ADR-0020 decisions 5 and 6).
//!
//! Pure: the caller loads the unmatched lines of both sides for one account
//! and stores what comes out. Matching never guesses:
//!
//! 1. **Exact.** A ledger line whose entry's `bank_ref` is the statement
//!    line's ID, with the same signed amount. Several such ledger lines (a
//!    movement booked twice) match the first posted; the rest stay
//!    unmatched. The same `bank_ref` with another amount is an
//!    `amount_mismatch`, and both lines are held for it.
//! 2. **Rule.** The same signed amount, the same non-empty reference
//!    (`payment_ref` on the ledger side), booking dates at most the window
//!    apart, and exactly one candidate on each side. Anything less certain
//!    is left unmatched.
//!
//! What is left becomes a break once it is older than the grace period, and
//! is pending until then.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

/// One unmatched statement line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: Uuid,
    pub line_ref: String,
    /// Days since 1970-01-01.
    pub date: i32,
    /// Signed minor units: positive is money into the account.
    pub amount: i64,
    pub reference: String,
}

/// One unmatched ledger line of the reconciled account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerLine {
    pub entry: Uuid,
    pub line_no: i16,
    /// Posting order, to pick the first of several equal candidates.
    pub sequence: i64,
    /// The entry's event date (or recorded date), days since 1970-01-01.
    pub date: i32,
    /// Signed minor units: a debit is positive, a credit negative.
    pub amount: i64,
    /// The entry's `bank_ref` metadata: the bank's line ID.
    pub bank_ref: Option<String>,
    /// The entry's `payment_ref` metadata: the payment reference.
    pub payment_ref: Option<String>,
}

/// The matching windows, in days.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchOptions {
    /// How far apart the dates of a rule match may be.
    pub window_days: i32,
    /// How old an unmatched line must be before it is a break.
    pub grace_days: i32,
}

impl Default for MatchOptions {
    fn default() -> MatchOptions {
        MatchOptions {
            window_days: 3,
            grace_days: 2,
        }
    }
}

/// The booking dates the account's statements cover so far: ledger lines
/// before the first are older than reconciliation and are not reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coverage {
    pub first: i32,
    pub last: i32,
}

/// How a pair was matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    Exact,
    Rule,
}

impl Rule {
    pub fn as_str(self) -> &'static str {
        match self {
            Rule::Exact => "exact",
            Rule::Rule => "rule",
        }
    }
}

/// A disagreement found by matching, by index into the inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finding {
    /// The bank shows a movement the ledger does not.
    MissingInLedger { item: usize },
    /// A statement line that repeats another (same date, amount and
    /// reference) and has no ledger line of its own.
    Duplicate { item: usize },
    /// The ledger shows a movement the bank does not.
    MissingInStatement { line: usize },
    /// The same `bank_ref`, another amount.
    AmountMismatch { item: usize, line: usize },
}

/// What matching found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Matching {
    /// (item, line, rule), by index into the inputs.
    pub matches: Vec<(usize, usize, Rule)>,
    pub findings: Vec<Finding>,
    /// Unmatched, but younger than the grace period.
    pub pending_items: Vec<usize>,
    pub pending_lines: Vec<usize>,
}

/// The key that makes two statement lines look like the same movement.
pub type DuplicateKey = (i32, i64, String);

/// Matches `items` with `lines`, both unmatched and of one account.
/// `matched_keys` are the keys of the account's statement lines already
/// matched, so a repeat of one is classified as a duplicate. `today` is the
/// run's date, in days.
pub fn match_lines(
    items: &[Item],
    lines: &[LedgerLine],
    matched_keys: &HashSet<DuplicateKey>,
    coverage: Option<Coverage>,
    today: i32,
    options: MatchOptions,
) -> Matching {
    let mut result = Matching::default();
    let mut item_used = vec![false; items.len()];
    let mut line_used = vec![false; lines.len()];

    // Deterministic order: items by date then line ID, lines by posting
    // order.
    let mut item_order: Vec<usize> = (0..items.len()).collect();
    item_order.sort_by(|&a, &b| {
        (items[a].date, &items[a].line_ref).cmp(&(items[b].date, &items[b].line_ref))
    });
    let mut line_order: Vec<usize> = (0..lines.len()).collect();
    line_order.sort_by_key(|&l| (lines[l].sequence, lines[l].line_no));

    // 1. Exact, by the bank's line ID.
    let mut by_bank_ref: HashMap<&str, Vec<usize>> = HashMap::new();
    for &l in &line_order {
        if let Some(bank_ref) = lines[l].bank_ref.as_deref().filter(|r| !r.is_empty()) {
            by_bank_ref.entry(bank_ref).or_default().push(l);
        }
    }
    for &i in &item_order {
        let Some(candidates) = by_bank_ref.get(items[i].line_ref.as_str()) else {
            continue;
        };
        let free: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&l| !line_used[l])
            .collect();
        if let Some(&l) = free.iter().find(|&&l| lines[l].amount == items[i].amount) {
            result.matches.push((i, l, Rule::Exact));
            item_used[i] = true;
            line_used[l] = true;
        } else if let Some(&l) = free.first() {
            result
                .findings
                .push(Finding::AmountMismatch { item: i, line: l });
            item_used[i] = true;
            line_used[l] = true;
        }
    }

    // 2. Rule: amount, reference and date, unique on both sides.
    let mut by_key: HashMap<(i64, &str), Vec<usize>> = HashMap::new();
    for &l in &line_order {
        if line_used[l] {
            continue;
        }
        if let Some(reference) = lines[l].payment_ref.as_deref().filter(|r| !r.is_empty()) {
            by_key
                .entry((lines[l].amount, reference))
                .or_default()
                .push(l);
        }
    }
    let mut candidates_of_item: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut items_per_line: HashMap<usize, usize> = HashMap::new();
    for &i in &item_order {
        if item_used[i] || items[i].reference.is_empty() {
            continue;
        }
        let candidates: Vec<usize> = by_key
            .get(&(items[i].amount, items[i].reference.as_str()))
            .into_iter()
            .flatten()
            .copied()
            .filter(|&l| (lines[l].date - items[i].date).abs() <= options.window_days)
            .collect();
        for &l in &candidates {
            *items_per_line.entry(l).or_default() += 1;
        }
        candidates_of_item.push((i, candidates));
    }
    for (i, candidates) in candidates_of_item {
        if let [l] = candidates[..]
            && items_per_line.get(&l) == Some(&1)
        {
            result.matches.push((i, l, Rule::Rule));
            item_used[i] = true;
            line_used[l] = true;
        }
    }

    // 3. What is left: a break once it is old enough.
    let mut seen: HashSet<DuplicateKey> = matched_keys.clone();
    for &(i, ..) in &result.matches {
        seen.insert(key(&items[i]));
    }
    for &i in &item_order {
        if item_used[i] {
            continue;
        }
        if today - items[i].date < options.grace_days {
            result.pending_items.push(i);
        } else if !seen.insert(key(&items[i])) {
            result.findings.push(Finding::Duplicate { item: i });
        } else {
            result.findings.push(Finding::MissingInLedger { item: i });
        }
    }
    if let Some(coverage) = coverage {
        for &l in &line_order {
            if line_used[l] || lines[l].date < coverage.first {
                continue;
            }
            if coverage.last - lines[l].date < options.grace_days {
                result.pending_lines.push(l);
            } else {
                result
                    .findings
                    .push(Finding::MissingInStatement { line: l });
            }
        }
    }
    result
}

fn key(item: &Item) -> DuplicateKey {
    (item.date, item.amount, item.reference.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn item(n: u128, line_ref: &str, date: i32, amount: i64, reference: &str) -> Item {
        Item {
            id: Uuid::from_u128(n),
            line_ref: line_ref.into(),
            date,
            amount,
            reference: reference.into(),
        }
    }

    fn line(
        n: u128,
        date: i32,
        amount: i64,
        bank_ref: Option<&str>,
        payment_ref: Option<&str>,
    ) -> LedgerLine {
        LedgerLine {
            entry: Uuid::from_u128(n),
            line_no: 0,
            sequence: i64::try_from(n).unwrap_or_default(),
            date,
            amount,
            bank_ref: bank_ref.map(Into::into),
            payment_ref: payment_ref.map(Into::into),
        }
    }

    const DAY: i32 = 20_000;
    const COVER: Option<Coverage> = Some(Coverage {
        first: DAY,
        last: DAY + 10,
    });

    fn run(items: &[Item], lines: &[LedgerLine]) -> Matching {
        match_lines(
            items,
            lines,
            &HashSet::new(),
            COVER,
            DAY + 10,
            MatchOptions::default(),
        )
    }

    #[test]
    fn exact_then_rule() {
        let items = [
            item(1, "B1", DAY, 1000, "HANA"),
            item(2, "B2", DAY, 500, "ABEBE"),
        ];
        let lines = [
            line(1, DAY + 1, 500, None, Some("ABEBE")),
            line(2, DAY, 1000, Some("B1"), None),
        ];
        let m = run(&items, &lines);
        assert_eq!(m.matches, [(0, 1, Rule::Exact), (1, 0, Rule::Rule)]);
        assert!(m.findings.is_empty());
    }

    #[test]
    fn two_equal_candidates_are_never_picked_between() {
        let items = [item(1, "B1", DAY, 500, "REF")];
        let lines = [
            line(1, DAY, 500, None, Some("REF")),
            line(2, DAY + 1, 500, None, Some("REF")),
        ];
        let m = run(&items, &lines);
        assert!(m.matches.is_empty());
        assert_eq!(
            m.findings,
            [
                Finding::MissingInLedger { item: 0 },
                Finding::MissingInStatement { line: 0 },
                Finding::MissingInStatement { line: 1 },
            ]
        );
        // Nor two statement lines for one ledger line.
        let items = [
            item(1, "B1", DAY, 500, "REF"),
            item(2, "B2", DAY, 500, "REF"),
        ];
        let m = run(&items, &lines[..1]);
        assert!(m.matches.is_empty());
    }

    #[test]
    fn rule_needs_reference_amount_and_window() {
        let items = [item(1, "B1", DAY, 500, "REF")];
        for other in [
            line(1, DAY, 500, None, None),
            line(1, DAY, 501, None, Some("REF")),
            line(1, DAY, -500, None, Some("REF")),
            line(1, DAY + 4, 500, None, Some("REF")),
            line(1, DAY, 500, None, Some("ref")),
        ] {
            assert!(
                run(&items, std::slice::from_ref(&other)).matches.is_empty(),
                "{other:?}"
            );
        }
        let near = line(1, DAY - 3, 500, None, Some("REF"));
        assert_eq!(run(&items, &[near]).matches.len(), 1);
    }

    #[test]
    fn booked_twice_matches_the_first_and_reports_the_second() {
        let items = [item(1, "B1", DAY, 20_000, "")];
        let lines = [
            line(7, DAY, 20_000, Some("B1"), None),
            line(9, DAY, 20_000, Some("B1"), None),
        ];
        let m = run(&items, &lines);
        assert_eq!(m.matches, [(0, 0, Rule::Exact)]);
        assert_eq!(m.findings, [Finding::MissingInStatement { line: 1 }]);
    }

    #[test]
    fn same_bank_ref_other_amount_is_a_mismatch() {
        let items = [item(1, "B1", DAY, 19_000, "")];
        let lines = [line(1, DAY, 20_000, Some("B1"), None)];
        let m = run(&items, &lines);
        assert!(m.matches.is_empty());
        assert_eq!(m.findings, [Finding::AmountMismatch { item: 0, line: 0 }]);
    }

    #[test]
    fn a_repeated_statement_line_is_a_duplicate() {
        let items = [
            item(1, "B1", DAY, 300, "X"),
            item(2, "B2", DAY, 300, "X"),
            item(3, "B3", DAY + 1, 300, "X"),
        ];
        let lines = [line(1, DAY, 300, Some("B1"), None)];
        let m = run(&items, &lines);
        assert_eq!(m.matches, [(0, 0, Rule::Exact)]);
        assert_eq!(
            m.findings,
            [
                Finding::Duplicate { item: 1 },
                Finding::MissingInLedger { item: 2 },
            ]
        );
        // A repeat of a line matched in an earlier run, too.
        let earlier = HashSet::from([(DAY, 300, "X".to_owned())]);
        let m = match_lines(
            &items[1..2],
            &[],
            &earlier,
            COVER,
            DAY + 10,
            MatchOptions::default(),
        );
        assert_eq!(m.findings, [Finding::Duplicate { item: 0 }]);
    }

    #[test]
    fn young_lines_wait_and_old_ledger_lines_are_ignored() {
        let items = [item(1, "B1", DAY + 9, 100, "")];
        let lines = [
            line(1, DAY - 1, 100, None, None),
            line(2, DAY + 9, 200, None, None),
            line(3, DAY + 2, 300, None, None),
        ];
        let m = run(&items, &lines);
        assert_eq!(m.pending_items, [0]);
        assert_eq!(m.pending_lines, [1]);
        assert_eq!(m.findings, [Finding::MissingInStatement { line: 2 }]);
        // No statement yet: no ledger line is reported.
        let m = match_lines(
            &[],
            &lines,
            &HashSet::new(),
            None,
            DAY + 10,
            MatchOptions::default(),
        );
        assert!(m.findings.is_empty() && m.pending_lines.is_empty());
    }

    /// One real movement and how it shows on each side.
    #[derive(Debug, Clone)]
    struct Movement {
        date: i32,
        amount: i64,
        reference: String,
        /// The client recorded the bank's line ID.
        with_bank_ref: bool,
        /// The client recorded the payment reference.
        with_payment_ref: bool,
        /// Planted break: 0 none, 1 missing from the statement, 2 missing
        /// from the ledger, 3 another amount on the statement, 4 repeated
        /// on the statement.
        plant: u8,
    }

    fn movement() -> impl Strategy<Value = Movement> {
        (
            0..20i32,
            prop_oneof![Just(-1i64), Just(1i64)],
            1..6i64,
            0..4u8,
            any::<bool>(),
            any::<bool>(),
            prop_oneof![8 => Just(0u8), 1 => Just(1u8), 1 => Just(2u8), 1 => Just(3u8), 1 => Just(4u8)],
        )
            .prop_map(|(date, sign, amount, reference, with_bank_ref, with_payment_ref, plant)| {
                Movement {
                    date: DAY + date,
                    amount: sign * amount * 100,
                    reference: format!("R{reference}"),
                    with_bank_ref,
                    with_payment_ref,
                    plant,
                }
            })
    }

    proptest! {
        /// The acceptance criterion: every planted break is found, of its
        /// kind, and no pair from two different movements is ever matched.
        #[test]
        fn every_planted_break_is_found_and_nothing_is_falsely_matched(
            movements in prop::collection::vec(movement(), 0..60)
        ) {
            let mut items = Vec::new();
            let mut item_movement = Vec::new();
            let mut lines = Vec::new();
            let mut line_movement = Vec::new();
            // A planted break has its own reference, as two different
            // payments would: one statement line and one ledger line with
            // the same amount and reference within the window are, by the
            // rule's definition, the same movement. Clean movements share a
            // few references, so the rule meets real ambiguity.
            let movements: Vec<Movement> = movements
                .into_iter()
                .enumerate()
                .map(|(n, m)| Movement {
                    reference: if m.plant == 0 { m.reference } else { format!("P{n}") },
                    ..m
                })
                .collect();
            for (n, m) in movements.iter().enumerate() {
                let bank_ref = format!("B{n}");
                if m.plant != 1 {
                    let amount = if m.plant == 3 { m.amount + m.amount.signum() } else { m.amount };
                    items.push(item(n as u128, &bank_ref, m.date, amount, &m.reference));
                    item_movement.push(n);
                }
                if m.plant == 4 {
                    items.push(item(1000 + n as u128, &format!("B{n}x"), m.date, m.amount, &m.reference));
                    item_movement.push(n);
                }
                if m.plant != 2 {
                    lines.push(line(
                        n as u128,
                        m.date,
                        m.amount,
                        // An amount mismatch is found through the bank's ID.
                        (m.with_bank_ref || m.plant == 3).then_some(bank_ref.as_str()),
                        m.with_payment_ref.then_some(m.reference.as_str()),
                    ));
                    line_movement.push(n);
                }
            }
            let coverage = Some(Coverage { first: DAY, last: DAY + 30 });
            let result = match_lines(
                &items, &lines, &HashSet::new(), coverage, DAY + 30, MatchOptions::default(),
            );

            // Zero false matches.
            for &(i, l, _) in &result.matches {
                prop_assert_eq!(item_movement[i], line_movement[l]);
            }
            prop_assert!(result.pending_items.is_empty() && result.pending_lines.is_empty());

            // Every planted break is found, as its kind. A clean movement
            // may still be reported when it cannot be told apart from
            // another: a break, never a wrong match.
            for (n, m) in movements.iter().enumerate() {
                let item_of = |extra: bool| {
                    items.iter().position(|it| it.id == Uuid::from_u128(if extra { 1000 } else { 0 } + n as u128))
                };
                let line_of = || line_movement.iter().position(|&x| x == n);
                let found = |f: Finding| result.findings.contains(&f);
                match m.plant {
                    1 => {
                        let l = line_of().ok_or_else(|| TestCaseError::fail("line"))?;
                        let ok = found(Finding::MissingInStatement { line: l });
                        prop_assert!(ok, "movement {} missing from the statement", n);
                    }
                    2 => {
                        let i = item_of(false).ok_or_else(|| TestCaseError::fail("item"))?;
                        let ok = found(Finding::MissingInLedger { item: i })
                            || found(Finding::Duplicate { item: i });
                        prop_assert!(ok, "movement {} missing from the ledger", n);
                    }
                    3 => {
                        let i = item_of(false).ok_or_else(|| TestCaseError::fail("item"))?;
                        let l = line_of().ok_or_else(|| TestCaseError::fail("line"))?;
                        let ok = found(Finding::AmountMismatch { item: i, line: l });
                        prop_assert!(ok, "movement {} with another amount", n);
                    }
                    4 => {
                        // One of the two copies is matched or reported; the
                        // other is always reported.
                        let a = item_of(false).ok_or_else(|| TestCaseError::fail("item"))?;
                        let b = item_of(true).ok_or_else(|| TestCaseError::fail("copy"))?;
                        let reported = |i: usize| {
                            found(Finding::Duplicate { item: i }) || found(Finding::MissingInLedger { item: i })
                        };
                        prop_assert!(reported(a) || reported(b), "movement {} repeated", n);
                    }
                    _ => {}
                }
            }
        }
    }
}
