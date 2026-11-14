//! Deposits, withdrawals, clearing and suspense (step 2.02).

#[path = "../../core/tests/common/mod.rs"]
mod common;

use std::collections::{BTreeMap, HashMap};

use common::{CHART, fixtures, trial_balance};
use mezgeb_core::{
    Account, AccountId, AccountRecord, Chart, Currency, EntryDraft, EntryId, LineDraft, MemLedger,
    Money, PostingError, Side, validate,
};
use mezgeb_ops::{OperationRequest, TEMPLATE_KEY, TemplateError, TemplateRegistry};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn uuid7(n: u64) -> String {
    format!("00000000-0000-7000-8000-{n:012x}")
}

fn id(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    uuid7(n).parse()
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

fn request(template: &str, roles: &[(&str, AccountId)], amount: Money) -> OperationRequest {
    OperationRequest {
        template: template.into(),
        accounts: roles.iter().map(|(r, a)| ((*r).to_owned(), *a)).collect(),
        amounts: BTreeMap::from([("amount".into(), amount)]),
        ..OperationRequest::default()
    }
}

/// The template that books each worked entry of this step, and its roles in
/// the order of the entry's lines.
const BOOKED: &[(&str, &str, [&str; 2])] = &[
    (
        "E01 #1",
        "deposit_confirmed.v1",
        ["customer_funds", "customer_wallet"],
    ),
    (
        "E02 #1",
        "withdrawal_request.v1",
        ["customer_wallet", "pending_withdrawal"],
    ),
    (
        "E03 #1",
        "withdrawal_sent.v1",
        ["pending_withdrawal", "withdrawal_in_transit"],
    ),
    (
        "E03 #2",
        "withdrawal_confirmed.v1",
        ["withdrawal_in_transit", "customer_funds"],
    ),
    (
        "E04 #1",
        "withdrawal_request.v1",
        ["customer_wallet", "pending_withdrawal"],
    ),
    (
        "E04 #2",
        "withdrawal_sent.v1",
        ["pending_withdrawal", "withdrawal_in_transit"],
    ),
    (
        "E04 #3",
        "withdrawal_failed.v1",
        ["withdrawal_in_transit", "customer_wallet"],
    ),
    (
        "E05 #1",
        "deposit_confirmed.v1",
        ["customer_funds", "customer_wallet"],
    ),
    (
        "E19 #1",
        "deposit_unmatched.v1",
        ["customer_funds", "suspense_item"],
    ),
    ("E20 #1", "suspense_resolve.v1", ["suspense_item", "owner"]),
];

/// The whole worked story, with every entry of this step booked by its
/// template instead of by hand: each template's draft equals the entry in
/// the docs, line for line, and the story still ends at the trial balance.
#[test]
fn each_template_books_its_worked_entry() -> TestResult {
    let chart = Chart::from_toml(CHART)?;
    let fixtures = fixtures()?;
    let registry = TemplateRegistry::standard();
    let mut ledger = MemLedger::new();
    let mut ids = BTreeMap::new();
    for line in fixtures.iter().flat_map(|f| &f.lines) {
        if !ids.contains_key(&line.reference) {
            let account = id(ids.len() as u64)?;
            ledger.open_account(account, &chart, &line.reference)?;
            ids.insert(line.reference.clone(), account);
        }
    }

    let mut booked = 0;
    for (n, fixture) in fixtures.iter().enumerate() {
        let by_hand = EntryDraft {
            lines: fixture
                .lines
                .iter()
                .map(|l| LineDraft {
                    account: ids[&l.reference],
                    side: l.side,
                    amount: l.amount,
                })
                .collect(),
            ..EntryDraft::default()
        };
        let draft = match BOOKED.iter().find(|(label, ..)| *label == fixture.label) {
            None => by_hand,
            Some((label, template, roles)) => {
                let bound: Vec<(&str, AccountId)> = roles
                    .iter()
                    .zip(&fixture.lines)
                    .map(|(role, line)| (*role, ids[&line.reference]))
                    .collect();
                let draft = registry
                    .draft(&request(template, &bound, fixture.total), &ledger)
                    .map_err(|e| format!("{label}: {e}"))?;
                assert_eq!(draft.lines, by_hand.lines, "{label}");
                assert_eq!(draft.metadata[TEMPLATE_KEY], *template, "{label}");
                booked += 1;
                draft
            }
        };
        ledger
            .post(EntryId::from_uuid(id(10_000 + n as u64)?.as_uuid())?, draft)
            .map_err(|e| format!("{}: {e}", fixture.label))?;
    }
    assert_eq!(booked, BOOKED.len(), "every listed entry was booked");

    for (reference, balance) in trial_balance()? {
        let account = ledger.account(ids[&reference]).ok_or("missing account")?;
        assert_eq!(account.balance(), balance, "{reference}");
    }
    Ok(())
}

/// A ledger of plain accounts, cheap enough for thousands of sub-accounts:
/// every posting is validated, then applied to the totals.
struct Books {
    chart: Chart,
    accounts: HashMap<AccountId, Account>,
    next: u64,
    /// Entries posted so far.
    posted: usize,
    registry: TemplateRegistry,
}

impl Books {
    fn new() -> Result<Books, Box<dyn std::error::Error>> {
        Ok(Books {
            chart: Chart::from_toml(CHART)?,
            accounts: HashMap::new(),
            next: 1,
            posted: 0,
            registry: TemplateRegistry::standard(),
        })
    }

    fn open(&mut self, reference: &str) -> Result<AccountId, Box<dyn std::error::Error>> {
        let account_id = id(self.next)?;
        self.next += 1;
        let account = Account::open(account_id, &self.chart, reference)?;
        self.accounts.insert(account_id, account);
        Ok(account_id)
    }

    fn balance(&self, account: AccountId) -> i64 {
        self.accounts
            .get(&account)
            .map_or(0, |a| a.balance().amount())
    }

    fn post(&mut self, draft: EntryDraft) -> Result<(), PostingError> {
        let entry = validate(draft, &self.accounts)?;
        self.posted += 1;
        for line in entry.lines() {
            let Some(account) = self.accounts.get(&line.account()) else {
                continue;
            };
            let amount = line.amount().amount();
            let (debits, credits) = match line.side() {
                Side::Debit => (account.debits_total() + amount, account.credits_total()),
                Side::Credit => (account.debits_total(), account.credits_total() + amount),
            };
            let record = AccountRecord {
                id: account.id(),
                code: account.code(),
                sub_account: account.sub_account().map(str::to_owned),
                name: account.name().to_owned(),
                account_type: account.account_type(),
                currency: account.currency(),
                allow_negative: account.allow_negative(),
                status: account.status(),
                debits_total: debits,
                credits_total: credits,
            };
            if let Ok(updated) = Account::from_record(record) {
                self.accounts.insert(updated.id(), updated);
            }
        }
        Ok(())
    }

    /// Drafts with the standard templates and posts.
    fn book(
        &mut self,
        template: &str,
        roles: &[(&str, AccountId)],
        amount: i64,
    ) -> Result<(), Refused> {
        let draft = self
            .registry
            .draft(&request(template, roles, etb(amount)), &self.accounts)
            .map_err(Refused::Template)?;
        self.post(draft).map_err(Refused::Posting)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Refused {
    Template(TemplateError),
    Posting(PostingError),
}

impl Refused {
    fn code(&self) -> u16 {
        match self {
            Refused::Template(e) => e.code(),
            Refused::Posting(e) => e.code(),
        }
    }
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Template(e) => e.fmt(f),
            Refused::Posting(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Refused {}

/// A customer with 1,000.00 ETB deposited, and the bank account.
struct Setup {
    books: Books,
    bank: AccountId,
    wallet: AccountId,
}

fn funded() -> Result<Setup, Box<dyn std::error::Error>> {
    let mut books = Books::new()?;
    let bank = books.open("1010")?;
    let wallet = books.open("2010:cust-abebe")?;
    books.book(
        "deposit_confirmed.v1",
        &[("customer_funds", bank), ("customer_wallet", wallet)],
        100_000,
    )?;
    Ok(Setup {
        books,
        bank,
        wallet,
    })
}

/// The withdrawal's two sub-accounts.
fn withdrawal(
    books: &mut Books,
    n: u32,
) -> Result<(AccountId, AccountId), Box<dyn std::error::Error>> {
    Ok((
        books.open(&format!("2020:wd-{n:04}"))?,
        books.open(&format!("9030:wdt-{n:04}"))?,
    ))
}

#[test]
fn the_happy_path_balances_at_every_step() -> TestResult {
    let Setup {
        mut books,
        bank,
        wallet,
    } = funded()?;
    assert_eq!(
        (books.balance(bank), books.balance(wallet)),
        (100_000, 100_000)
    );
    let (pending, transit) = withdrawal(&mut books, 1)?;

    books.book(
        "withdrawal_request.v1",
        &[("customer_wallet", wallet), ("pending_withdrawal", pending)],
        30_000,
    )?;
    assert_eq!(books.balance(wallet), 70_000);
    assert_eq!(books.balance(pending), 30_000);
    assert_eq!(books.balance(bank), 100_000);

    books.book(
        "withdrawal_sent.v1",
        &[
            ("pending_withdrawal", pending),
            ("withdrawal_in_transit", transit),
        ],
        30_000,
    )?;
    assert_eq!(books.balance(pending), 0);
    assert_eq!(books.balance(transit), 30_000);
    assert_eq!(
        books.balance(bank),
        100_000,
        "cash leaves only on confirmation"
    );

    books.book(
        "withdrawal_confirmed.v1",
        &[("withdrawal_in_transit", transit), ("customer_funds", bank)],
        30_000,
    )?;
    assert_eq!(books.balance(transit), 0, "clearing nets to zero");
    assert_eq!(books.balance(bank), 70_000);
    assert_eq!(books.balance(wallet), 70_000);
    Ok(())
}

#[test]
fn a_failed_withdrawal_returns_exactly_the_original_amount() -> TestResult {
    let Setup {
        mut books,
        bank,
        wallet,
    } = funded()?;
    let (pending, transit) = withdrawal(&mut books, 1)?;
    books.book(
        "withdrawal_request.v1",
        &[("customer_wallet", wallet), ("pending_withdrawal", pending)],
        12_345,
    )?;
    books.book(
        "withdrawal_sent.v1",
        &[
            ("pending_withdrawal", pending),
            ("withdrawal_in_transit", transit),
        ],
        12_345,
    )?;

    // Not one santim more or less than what is in transit.
    let failed = |amount| {
        request(
            "withdrawal_failed.v1",
            &[
                ("withdrawal_in_transit", transit),
                ("customer_wallet", wallet),
            ],
            etb(amount),
        )
    };
    let registry = TemplateRegistry::standard();
    for wrong in [12_344, 12_346, 1] {
        assert_eq!(
            registry.draft(&failed(wrong), &books.accounts),
            Err(TemplateError::AmountNotBalance(
                "withdrawal_in_transit".into()
            ))
        );
    }

    books.book(
        "withdrawal_failed.v1",
        &[
            ("withdrawal_in_transit", transit),
            ("customer_wallet", wallet),
        ],
        12_345,
    )?;
    assert_eq!(books.balance(wallet), 100_000, "the wallet is whole again");
    assert_eq!(books.balance(transit), 0, "clearing nets to zero");
    assert_eq!(books.balance(pending), 0);
    assert_eq!(books.balance(bank), 100_000, "the cash never left");
    Ok(())
}

/// Sets up one withdrawal in transit and returns the two outcome requests.
fn in_transit() -> Result<(Books, OperationRequest, OperationRequest), Box<dyn std::error::Error>> {
    let Setup {
        mut books,
        bank,
        wallet,
    } = funded()?;
    let (pending, transit) = withdrawal(&mut books, 1)?;
    books.book(
        "withdrawal_request.v1",
        &[("customer_wallet", wallet), ("pending_withdrawal", pending)],
        20_000,
    )?;
    books.book(
        "withdrawal_sent.v1",
        &[
            ("pending_withdrawal", pending),
            ("withdrawal_in_transit", transit),
        ],
        20_000,
    )?;
    let confirm = request(
        "withdrawal_confirmed.v1",
        &[("withdrawal_in_transit", transit), ("customer_funds", bank)],
        etb(20_000),
    );
    let fail = request(
        "withdrawal_failed.v1",
        &[
            ("withdrawal_in_transit", transit),
            ("customer_wallet", wallet),
        ],
        etb(20_000),
    );
    Ok((books, confirm, fail))
}

#[test]
fn confirm_after_fail_and_fail_after_confirm_are_rejected() -> TestResult {
    let registry = TemplateRegistry::standard();
    for (first, second) in [(1, 0), (0, 1), (0, 0), (1, 1)] {
        let (mut books, confirm, fail) = in_transit()?;
        let outcomes = [&confirm, &fail];
        // Both outcomes drafted while the payout is in transit, as two
        // racing bank notices would be.
        let stale = registry.draft(outcomes[second], &books.accounts)?;
        books.post(registry.draft(outcomes[first], &books.accounts)?)?;

        // The template refuses the second outcome...
        assert_eq!(
            registry.draft(outcomes[second], &books.accounts),
            Err(TemplateError::AmountNotBalance(
                "withdrawal_in_transit".into()
            )),
            "{first} then {second}"
        );
        // ...and the posting rules refuse it even when it was drafted before
        // the first outcome posted.
        assert_eq!(
            books.post(stale),
            Err(PostingError::InsufficientFunds { line: 0 }),
            "{first} then {second}"
        );
    }
    Ok(())
}

#[test]
fn each_step_refuses_what_breaks_the_policy() -> TestResult {
    let Setup {
        mut books,
        bank,
        wallet,
    } = funded()?;
    let (pending, transit) = withdrawal(&mut books, 1)?;
    let merchant = books.open("2030:merch-coffee")?;
    let request_wd = |amount| {
        (
            "withdrawal_request.v1",
            vec![("customer_wallet", wallet), ("pending_withdrawal", pending)],
            amount,
        )
    };

    // Sending before the request: nothing is pending.
    let send = (
        "withdrawal_sent.v1",
        vec![
            ("pending_withdrawal", pending),
            ("withdrawal_in_transit", transit),
        ],
    );
    assert_eq!(
        books.book(send.0, &send.1, 5_000).map_err(|e| e.code()),
        Err(2014)
    );

    let (t, roles, amount) = request_wd(5_000);
    books.book(t, &roles, amount)?;
    // The same pending withdrawal cannot be requested twice (2015).
    let (t, roles, amount) = request_wd(5_000);
    assert_eq!(
        books.book(t, &roles, amount).map_err(|e| e.code()),
        Err(2015)
    );
    // More than the wallet holds: the posting rules refuse it (1012).
    let (other, _) = withdrawal(&mut books, 2)?;
    assert_eq!(
        books
            .book(
                "withdrawal_request.v1",
                &[("customer_wallet", wallet), ("pending_withdrawal", other)],
                100_001,
            )
            .map_err(|e| e.code()),
        Err(1012)
    );
    // Sending part of it is refused (2014); so is sending to a bank account
    // (2010), or into a transit item that is not new (2015).
    assert_eq!(
        books.book(send.0, &send.1, 4_999).map_err(|e| e.code()),
        Err(2014)
    );
    assert_eq!(
        books
            .book(
                "withdrawal_sent.v1",
                &[
                    ("pending_withdrawal", pending),
                    ("withdrawal_in_transit", bank)
                ],
                5_000
            )
            .map_err(|e| e.code()),
        Err(2010)
    );
    books.book(send.0, &send.1, 5_000)?;
    // Sent twice: the transit item is no longer new (2015), and nothing is
    // pending any more.
    assert_eq!(
        books.book(send.0, &send.1, 5_000).map_err(|e| e.code()),
        Err(2015)
    );
    // A new pending withdrawal cannot reuse a used transit item (2015).
    books.book(
        "withdrawal_request.v1",
        &[("customer_wallet", wallet), ("pending_withdrawal", other)],
        1_000,
    )?;
    assert_eq!(
        books
            .book(
                "withdrawal_sent.v1",
                &[
                    ("pending_withdrawal", other),
                    ("withdrawal_in_transit", transit)
                ],
                1_000
            )
            .map_err(|e| e.code()),
        Err(2015)
    );
    // A confirmation must name the customer funds account (2010).
    assert_eq!(
        books
            .book(
                "withdrawal_confirmed.v1",
                &[
                    ("withdrawal_in_transit", transit),
                    ("customer_funds", merchant)
                ],
                5_000
            )
            .map_err(|e| e.code()),
        Err(2010)
    );

    // Suspense: a new item per deposit (2015), resolved whole (2014), only
    // to a customer or merchant (2010), and only once.
    let item = books.open("9010:sus-0001")?;
    let unmatched = [("customer_funds", bank), ("suspense_item", item)];
    books.book("deposit_unmatched.v1", &unmatched, 5_000)?;
    assert_eq!(
        books
            .book("deposit_unmatched.v1", &unmatched, 5_000)
            .map_err(|e| e.code()),
        Err(2015)
    );
    let resolve = |owner| [("suspense_item", item), ("owner", owner)];
    assert_eq!(
        books
            .book("suspense_resolve.v1", &resolve(wallet), 4_000)
            .map_err(|e| e.code()),
        Err(2014)
    );
    assert_eq!(
        books
            .book("suspense_resolve.v1", &resolve(bank), 5_000)
            .map_err(|e| e.code()),
        Err(2010)
    );
    books.book("suspense_resolve.v1", &resolve(merchant), 5_000)?;
    assert_eq!(books.balance(merchant), 5_000);
    assert_eq!(books.balance(item), 0);
    assert_eq!(
        books
            .book("suspense_resolve.v1", &resolve(merchant), 5_000)
            .map_err(|e| e.code()),
        Err(2014)
    );
    Ok(())
}

/// A small, fast random number generator, so the run is the same every time.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[derive(Debug, Clone, Copy)]
struct Withdrawal {
    customer: usize,
    pending: AccountId,
    transit: AccountId,
    amount: i64,
}

/// The acceptance criterion: after 10,000 random deposit and withdrawal
/// lifecycles, clearing equals the sum of the withdrawals in flight, at
/// every step. Outcomes that break the lifecycle (a second outcome, a
/// partial send, a double request) are tried along the way and must all be
/// refused without changing anything.
#[test]
fn clearing_equals_the_withdrawals_in_flight_after_10k_lifecycles() -> TestResult {
    const LIFECYCLES: usize = 10_000;
    const CUSTOMERS: usize = 8;
    let mut rng = Rng(0x2002_0001);
    let mut books = Books::new()?;
    let bank = books.open("1010")?;
    let merchant = books.open("2030:merch-shop")?;
    let mut wallets = Vec::new();
    for c in 0..CUSTOMERS {
        wallets.push(books.open(&format!("2010:cust-{c}"))?);
    }

    let mut next_item = 0u32;
    let mut pending: Vec<Withdrawal> = Vec::new();
    let mut transit: Vec<Withdrawal> = Vec::new();
    let mut finished: Vec<Withdrawal> = Vec::new();
    let mut suspense: Vec<(AccountId, i64)> = Vec::new();
    let mut completed = 0;
    let mut refused = 0;
    let mut cash: i64 = 0;

    while completed < LIFECYCLES {
        match rng.below(10) {
            // A deposit, matched or not.
            0 | 1 => {
                let amount = 1 + rng.below(50_000) as i64;
                if rng.below(4) == 0 {
                    next_item += 1;
                    let item = books.open(&format!("9010:sus-{next_item}"))?;
                    books.book(
                        "deposit_unmatched.v1",
                        &[("customer_funds", bank), ("suspense_item", item)],
                        amount,
                    )?;
                    suspense.push((item, amount));
                } else {
                    let wallet = wallets[rng.below(CUSTOMERS)];
                    books.book(
                        "deposit_confirmed.v1",
                        &[("customer_funds", bank), ("customer_wallet", wallet)],
                        amount,
                    )?;
                }
                cash += amount;
            }
            // A suspense item is resolved to a customer or the merchant.
            2 if !suspense.is_empty() => {
                let (item, amount) = suspense.swap_remove(rng.below(suspense.len()));
                let owner = if rng.below(5) == 0 {
                    merchant
                } else {
                    wallets[rng.below(CUSTOMERS)]
                };
                books.book(
                    "suspense_resolve.v1",
                    &[("suspense_item", item), ("owner", owner)],
                    amount,
                )?;
            }
            // A withdrawal is requested, for up to what the wallet holds.
            3 | 4 => {
                let customer = rng.below(CUSTOMERS);
                let wallet = wallets[customer];
                let balance = books.balance(wallet);
                if balance == 0 {
                    continue;
                }
                let amount = 1 + (rng.next() % balance as u64) as i64;
                next_item += 1;
                let w = Withdrawal {
                    customer,
                    pending: books.open(&format!("2020:wd-{next_item}"))?,
                    transit: books.open(&format!("9030:wdt-{next_item}"))?,
                    amount,
                };
                books.book(
                    "withdrawal_request.v1",
                    &[
                        ("customer_wallet", wallet),
                        ("pending_withdrawal", w.pending),
                    ],
                    amount,
                )?;
                pending.push(w);
            }
            // A pending withdrawal is sent.
            5 | 6 if !pending.is_empty() => {
                let w = pending.swap_remove(rng.below(pending.len()));
                books.book(
                    "withdrawal_sent.v1",
                    &[
                        ("pending_withdrawal", w.pending),
                        ("withdrawal_in_transit", w.transit),
                    ],
                    w.amount,
                )?;
                transit.push(w);
            }
            // The bank confirms or rejects a payout in transit.
            7 | 8 if !transit.is_empty() => {
                let w = transit.swap_remove(rng.below(transit.len()));
                if rng.below(4) == 0 {
                    books.book(
                        "withdrawal_failed.v1",
                        &[
                            ("withdrawal_in_transit", w.transit),
                            ("customer_wallet", wallets[w.customer]),
                        ],
                        w.amount,
                    )?;
                } else {
                    books.book(
                        "withdrawal_confirmed.v1",
                        &[
                            ("withdrawal_in_transit", w.transit),
                            ("customer_funds", bank),
                        ],
                        w.amount,
                    )?;
                    cash -= w.amount;
                }
                finished.push(w);
                completed += 1;
            }
            // Something that breaks the lifecycle; it must change nothing.
            _ => {
                // `validate` refuses before anything is applied, so a refused
                // step posts nothing.
                let before = books.posted;
                let attempt = match rng.below(4) {
                    0 if !finished.is_empty() => {
                        let w = finished[rng.below(finished.len())];
                        books.book(
                            "withdrawal_confirmed.v1",
                            &[
                                ("withdrawal_in_transit", w.transit),
                                ("customer_funds", bank),
                            ],
                            w.amount,
                        )
                    }
                    1 if !finished.is_empty() => {
                        let w = finished[rng.below(finished.len())];
                        books.book(
                            "withdrawal_failed.v1",
                            &[
                                ("withdrawal_in_transit", w.transit),
                                ("customer_wallet", wallets[w.customer]),
                            ],
                            w.amount,
                        )
                    }
                    2 if !pending.is_empty() => {
                        let w = pending[rng.below(pending.len())];
                        books.book(
                            "withdrawal_sent.v1",
                            &[
                                ("pending_withdrawal", w.pending),
                                ("withdrawal_in_transit", w.transit),
                            ],
                            w.amount + 1,
                        )
                    }
                    3 if !finished.is_empty() => {
                        let w = finished[rng.below(finished.len())];
                        books.book(
                            "withdrawal_request.v1",
                            &[
                                ("customer_wallet", wallets[w.customer]),
                                ("pending_withdrawal", w.pending),
                            ],
                            1,
                        )
                    }
                    _ => continue,
                };
                assert!(attempt.is_err(), "a broken lifecycle step was accepted");
                assert_eq!(books.posted, before, "a refused step posted");
                refused += 1;
            }
        }

        // Clearing equals the withdrawals in flight, and pending equals the
        // withdrawals requested and not yet sent.
        let in_transit: i64 = transit.iter().map(|w| w.amount).sum();
        let clearing: i64 = transit.iter().map(|w| books.balance(w.transit)).sum();
        assert_eq!(clearing, in_transit);
        let requested: i64 = pending.iter().map(|w| w.amount).sum();
        let pending_total: i64 = pending.iter().map(|w| books.balance(w.pending)).sum();
        assert_eq!(pending_total, requested);
    }

    // Across every account, not just the open items: 9030 is the money in
    // flight, 2020 the money pending, 9010 the unidentified money, and every
    // finished withdrawal is back at zero.
    let total = |code: u16| -> i64 {
        books
            .accounts
            .values()
            .filter(|a| a.code().value() == code)
            .map(|a| a.balance().amount())
            .sum()
    };
    assert_eq!(total(9030), transit.iter().map(|w| w.amount).sum::<i64>());
    assert_eq!(total(2020), pending.iter().map(|w| w.amount).sum::<i64>());
    assert_eq!(total(9010), suspense.iter().map(|(_, a)| a).sum::<i64>());
    for w in &finished {
        assert_eq!((books.balance(w.pending), books.balance(w.transit)), (0, 0));
    }
    // The bank account is what came in minus what the bank confirmed, and
    // it backs every customer, merchant, pending, in-transit and suspense
    // balance exactly (no fees in this step).
    assert_eq!(total(1010), cash);
    assert_eq!(
        total(1010),
        total(2010) + total(2020) + total(2030) + total(9010) + total(9030)
    );
    assert_eq!(finished.len(), LIFECYCLES);
    assert!(refused > 1_000, "only {refused} broken steps were tried");
    Ok(())
}
