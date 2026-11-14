//! Merchant payments, payment requests and fees (step 2.03).

#[path = "../../core/tests/common/mod.rs"]
mod common;
#[path = "../../engine/tests/support/mod.rs"]
mod support;

use std::collections::BTreeMap;

use common::{CHART, fixtures, trial_balance};
use mezgeb_core::{
    AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, Ledger, LineDraft, MemLedger, Money,
    Side,
};
use mezgeb_engine::{PgStore, PostOptions, integrity_report};
use mezgeb_ops::{CHANNELS, OperationRequest, TEMPLATE_KEY, TemplateError, TemplateRegistry};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn id(n: u64) -> Result<AccountId, mezgeb_core::IdError> {
    support::uuid7(n).parse()
}

fn entry(n: u64) -> Result<EntryId, mezgeb_core::IdError> {
    support::uuid7(1_000_000 + n).parse()
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

/// A payment request: the template, its roles, amounts and metadata.
fn payment(
    template: &str,
    roles: &[(&str, AccountId)],
    amounts: &[(&str, Money)],
    metadata: &[(&str, &str)],
) -> OperationRequest {
    OperationRequest {
        template: template.into(),
        accounts: roles.iter().map(|(r, a)| ((*r).to_owned(), *a)).collect(),
        amounts: amounts.iter().map(|(n, m)| ((*n).to_owned(), *m)).collect(),
        metadata: metadata
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        ..OperationRequest::default()
    }
}

/// The template that books each worked entry of this step, with the channel.
/// Each entry's lines are, in order: the payer, the payee, then the fee.
const BOOKED: &[(&str, &str, &str, &str)] = &[
    ("E07 #1", "merchant_payment.v1", "channel", "qr"),
    ("E08 #1", "merchant_payment.v1", "channel", "card_tap"),
    ("E09 #1", "merchant_payment.v1", "channel", "fingerprint"),
    (
        "E10 #1",
        "payment_request_settle.v1",
        "payment_request",
        "req-0001",
    ),
];

/// The whole worked story, with E07–E10 booked by their templates: each
/// draft equals the entry in the docs line for line, the story still ends at
/// the trial balance, and fee revenue there equals the fee lines posted.
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
        let lines: Vec<LineDraft> = fixture
            .lines
            .iter()
            .map(|l| LineDraft {
                account: ids[&l.reference],
                side: l.side,
                amount: l.amount,
            })
            .collect();
        let draft = match BOOKED.iter().find(|(label, ..)| *label == fixture.label) {
            None => EntryDraft {
                lines,
                ..EntryDraft::default()
            },
            Some((label, template, key, value)) => {
                let payee = if *template == "merchant_payment.v1" {
                    "merchant_balance"
                } else {
                    "payee"
                };
                let mut roles = vec![
                    ("payer_wallet", lines[0].account),
                    (payee, lines[1].account),
                ];
                let mut amounts = vec![("amount", fixture.total)];
                if let Some(fee) = lines.get(2) {
                    roles.push(("fee_revenue", fee.account));
                    amounts.push(("merchant_fee", fee.amount));
                }
                let request = payment(template, &roles, &amounts, &[(key, value)]);
                let draft = registry
                    .draft(&request, &ledger)
                    .map_err(|e| format!("{label}: {e}"))?;
                assert_eq!(draft.lines, lines, "{label}");
                assert_eq!(draft.metadata[TEMPLATE_KEY], *template, "{label}");
                assert_eq!(draft.metadata[*key], *value, "{label}");
                booked += 1;
                draft
            }
        };
        ledger
            .post(entry(n as u64)?, draft)
            .map_err(|e| format!("{}: {e}", fixture.label))?;
    }
    assert_eq!(booked, BOOKED.len());

    for (reference, balance) in trial_balance()? {
        let account = ledger.account(ids[&reference]).ok_or("missing account")?;
        assert_eq!(account.balance(), balance, "{reference}");
    }

    // Fee revenue in the trial balance is the fee lines posted to it: the
    // credits from payments, less the fees given back in refunds.
    let fee_revenue = ids["4010"];
    let fee_lines: i64 = ledger
        .history(fee_revenue)
        .iter()
        .map(|(_, line)| match line.side() {
            Side::Credit => line.amount().amount(),
            Side::Debit => -line.amount().amount(),
        })
        .sum();
    let in_trial_balance = trial_balance()?
        .into_iter()
        .find(|(reference, _)| reference == "4010")
        .map(|(_, balance)| balance.amount());
    assert_eq!(in_trial_balance, Some(fee_lines));
    Ok(())
}

/// A payer, a merchant and fee revenue, with 1,000.00 ETB in the wallet.
struct Shop {
    ledger: MemLedger,
    payer: AccountId,
    merchant: AccountId,
    fees: AccountId,
    registry: TemplateRegistry,
}

impl Shop {
    fn new() -> Result<Shop, Box<dyn std::error::Error>> {
        let chart = Chart::from_toml(CHART)?;
        let mut ledger = MemLedger::new();
        let (bank, payer, merchant, fees) = (id(1)?, id(2)?, id(3)?, id(4)?);
        ledger.open_account(bank, &chart, "1010")?;
        ledger.open_account(payer, &chart, "2010:cust-abebe")?;
        ledger.open_account(merchant, &chart, "2030:merch-coffee")?;
        ledger.open_account(fees, &chart, "4010")?;
        ledger.post(
            entry(0)?,
            EntryDraft::default()
                .debit(bank, etb(100_000))
                .credit(payer, etb(100_000)),
        )?;
        Ok(Shop {
            ledger,
            payer,
            merchant,
            fees,
            registry: TemplateRegistry::standard(),
        })
    }

    fn roles(&self) -> [(&'static str, AccountId); 3] {
        [
            ("payer_wallet", self.payer),
            ("merchant_balance", self.merchant),
            ("fee_revenue", self.fees),
        ]
    }

    /// The lines a merchant payment drafts, as (account, side, amount).
    fn lines(
        &self,
        amounts: &[(&str, Money)],
    ) -> Result<Vec<(AccountId, Side, i64)>, TemplateError> {
        let request = payment(
            "merchant_payment.v1",
            &self.roles(),
            amounts,
            &[("channel", "qr")],
        );
        Ok(self
            .registry
            .draft(&request, &self.ledger)?
            .lines
            .iter()
            .map(|l| (l.account, l.side, l.amount.amount()))
            .collect())
    }
}

#[test]
fn merchant_paid_and_payer_paid_fees() -> TestResult {
    let shop = Shop::new()?;
    let (payer, merchant, fees) = (shop.payer, shop.merchant, shop.fees);
    let d = Side::Debit;
    let c = Side::Credit;

    // The merchant pays the fee out of the price (E07).
    assert_eq!(
        shop.lines(&[("amount", etb(10_000)), ("merchant_fee", etb(200))])?,
        [(payer, d, 10_000), (merchant, c, 9_800), (fees, c, 200)]
    );
    // The payer pays the fee on top of the price.
    assert_eq!(
        shop.lines(&[("amount", etb(10_000)), ("payer_fee", etb(150))])?,
        [(payer, d, 10_150), (merchant, c, 10_000), (fees, c, 150)]
    );
    // Both at once: fee revenue gets both, in one line.
    assert_eq!(
        shop.lines(&[
            ("amount", etb(10_000)),
            ("merchant_fee", etb(200)),
            ("payer_fee", etb(150)),
        ])?,
        [(payer, d, 10_150), (merchant, c, 9_800), (fees, c, 350)]
    );
    // A fee equal to the amount leaves the merchant nothing, and no line.
    assert_eq!(
        shop.lines(&[("amount", etb(500)), ("merchant_fee", etb(500))])?,
        [(payer, d, 500), (fees, c, 500)]
    );
    Ok(())
}

#[test]
fn a_zero_fee_produces_no_fee_line() -> TestResult {
    let shop = Shop::new()?;
    let no_fee = [
        (shop.payer, Side::Debit, 10_000),
        (shop.merchant, Side::Credit, 10_000),
    ];
    // No fee given, or fees given as zero.
    assert_eq!(shop.lines(&[("amount", etb(10_000))])?, no_fee);
    assert_eq!(
        shop.lines(&[
            ("amount", etb(10_000)),
            ("merchant_fee", etb(0)),
            ("payer_fee", etb(0)),
        ])?,
        no_fee
    );
    // Without a fee, fee revenue need not be bound at all...
    let without_fee_revenue = payment(
        "merchant_payment.v1",
        &shop.roles()[..2],
        &[("amount", etb(10_000))],
        &[("channel", "card_tap")],
    );
    let draft = shop.registry.draft(&without_fee_revenue, &shop.ledger)?;
    assert_eq!(draft.lines.len(), 2);
    // ...but with one, it must be.
    let mut with_fee = without_fee_revenue;
    with_fee.amounts.insert("merchant_fee".into(), etb(1));
    assert_eq!(
        shop.registry.draft(&with_fee, &shop.ledger),
        Err(TemplateError::MissingAccount("fee_revenue".into()))
    );
    Ok(())
}

#[test]
fn fees_and_amounts_are_checked() -> TestResult {
    let shop = Shop::new()?;
    let code = |amounts: &[(&str, Money)]| shop.lines(amounts).map_err(|e| e.code());
    let usd = Money::from_minor(1, Currency::USD);
    type Case<'a> = (&'a str, &'a [(&'a str, Money)], u16);
    let cases: &[Case] = &[
        (
            "fee > amount",
            &[("amount", etb(100)), ("merchant_fee", etb(101))],
            2016,
        ),
        (
            "payer fee > amount",
            &[("amount", etb(100)), ("payer_fee", etb(101))],
            2016,
        ),
        (
            "fees together > amount",
            &[
                ("amount", etb(100)),
                ("merchant_fee", etb(60)),
                ("payer_fee", etb(41)),
            ],
            2016,
        ),
        (
            "negative fee",
            &[("amount", etb(100)), ("merchant_fee", etb(-1))],
            2006,
        ),
        ("zero amount", &[("amount", etb(0))], 2006),
        (
            "fee in another currency",
            &[("amount", etb(100)), ("payer_fee", usd)],
            2017,
        ),
        (
            "amount plus fee overflows",
            &[("amount", etb(i64::MAX)), ("payer_fee", etb(1))],
            2018,
        ),
        (
            "unknown amount",
            &[("amount", etb(100)), ("tip", etb(5))],
            2005,
        ),
    ];
    for (name, amounts, expected) in cases {
        assert_eq!(code(amounts), Err(*expected), "{name}");
    }
    // A fee of exactly the amount is allowed.
    assert!(code(&[("amount", etb(100)), ("merchant_fee", etb(100))]).is_ok());
    Ok(())
}

/// The exit gate: every channel in the product list (ADR-0014) books
/// through a template, and nothing else does.
#[test]
fn every_channel_books_through_a_template() -> TestResult {
    let shop = Shop::new()?;
    let amounts = [("amount", etb(1_000)), ("merchant_fee", etb(20))];
    assert_eq!(CHANNELS, ["qr", "card_tap", "phone_tap", "fingerprint"]);
    for channel in CHANNELS {
        let request = payment(
            "merchant_payment.v1",
            &shop.roles(),
            &amounts,
            &[("channel", channel)],
        );
        let draft = shop.registry.draft(&request, &shop.ledger)?;
        assert_eq!(draft.metadata["channel"], *channel);
        assert_eq!(draft.lines.len(), 3, "{channel}");
    }
    // The channel is required, and only these values are accepted.
    let check = |metadata: &[(&str, &str)]| {
        let request = payment("merchant_payment.v1", &shop.roles(), &amounts, metadata);
        shop.registry
            .draft(&request, &shop.ledger)
            .map_err(|e| e.code())
    };
    assert_eq!(check(&[]), Err(2019));
    for bad in ["nfc", "QR", "", "card tap"] {
        assert_eq!(check(&[("channel", bad)]), Err(2020), "{bad:?}");
    }
    // Other metadata is copied as it is.
    assert!(check(&[("channel", "qr"), ("order", "o-17")]).is_ok());

    // A payment request and a P2P transfer are channels too, each with its
    // own template.
    let hana = {
        let mut ledger = shop.ledger.clone();
        let chart = Chart::from_toml(CHART)?;
        let hana = id(5)?;
        ledger.open_account(hana, &chart, "2010:cust-hana")?;
        (ledger, hana)
    };
    let (ledger, hana) = hana;
    let settle = |metadata: &[(&str, &str)]| {
        let request = payment(
            "payment_request_settle.v1",
            &[("payer_wallet", shop.payer), ("payee", hana)],
            &[("amount", etb(8_000))],
            metadata,
        );
        shop.registry.draft(&request, &ledger)
    };
    assert_eq!(settle(&[]).map_err(|e| e.code()), Err(2019));
    let draft = settle(&[("payment_request", "req-0001")])?;
    assert_eq!(draft.metadata[TEMPLATE_KEY], "payment_request_settle.v1");
    assert_eq!(draft.lines.len(), 2);
    // A payment request can be paid to a merchant, with a fee, but never to
    // fee revenue or a bank account.
    let to_merchant = payment(
        "payment_request_settle.v1",
        &shop
            .roles()
            .map(|(r, a)| (if r == "merchant_balance" { "payee" } else { r }, a)),
        &amounts,
        &[("payment_request", "inv-9")],
    );
    assert_eq!(
        shop.registry.draft(&to_merchant, &shop.ledger)?.lines.len(),
        3
    );
    let mut to_fees = to_merchant;
    to_fees.accounts.insert("payee".into(), shop.fees);
    to_fees.accounts.remove("fee_revenue");
    assert_eq!(
        shop.registry
            .draft(&to_fees, &shop.ledger)
            .map_err(|e| e.code()),
        Err(2010)
    );
    Ok(())
}

/// A small, fast random number generator, so the run is the same every time.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// The acceptance criterion: fee revenue in the trial balance equals the
/// sum of the fee lines, over 2,000 random payments with every kind of fee
/// and every channel; and each merchant is owed exactly its net amounts.
#[test]
fn fee_revenue_equals_the_sum_of_the_fee_lines() -> TestResult {
    let chart = Chart::from_toml(CHART)?;
    let registry = TemplateRegistry::standard();
    let mut ledger = MemLedger::new();
    let bank = id(1)?;
    let fees = id(2)?;
    ledger.open_account(bank, &chart, "1010")?;
    ledger.open_account(fees, &chart, "4010")?;
    let mut payers = Vec::new();
    let mut merchants = Vec::new();
    for n in 0..10u64 {
        let payer = id(100 + n)?;
        ledger.open_account(payer, &chart, &format!("2010:cust-{n}"))?;
        ledger.post(
            entry(n)?,
            EntryDraft::default()
                .debit(bank, etb(10_000_000_000))
                .credit(payer, etb(10_000_000_000)),
        )?;
        payers.push(payer);
    }
    for n in 0..4u64 {
        let merchant = id(200 + n)?;
        ledger.open_account(merchant, &chart, &format!("2030:merch-{n}"))?;
        merchants.push(merchant);
    }

    let mut rng = Rng(0x2003);
    let mut requested_fees = 0;
    let mut net = vec![0i64; merchants.len()];
    for n in 0..2_000u64 {
        let amount = 1 + rng.below(1_000_000) as i64;
        let merchant_fee = if rng.below(3) == 0 {
            0
        } else {
            rng.below(amount as u64 / 10 + 1) as i64
        };
        // The fees together never exceed the amount (that is refused, and
        // tested separately).
        let payer_fee = if rng.below(3) == 0 {
            rng.below((amount - merchant_fee).min(500) as u64 + 1) as i64
        } else {
            0
        };
        let m = rng.below(merchants.len() as u64) as usize;
        let channel = CHANNELS[rng.below(CHANNELS.len() as u64) as usize];
        let payer = payers[rng.below(payers.len() as u64) as usize];
        let mut amounts = vec![("amount", etb(amount))];
        if merchant_fee > 0 || rng.below(2) == 0 {
            amounts.push(("merchant_fee", etb(merchant_fee)));
        }
        if payer_fee > 0 {
            amounts.push(("payer_fee", etb(payer_fee)));
        }
        let request = payment(
            "merchant_payment.v1",
            &[
                ("payer_wallet", payer),
                ("merchant_balance", merchants[m]),
                ("fee_revenue", fees),
            ],
            &amounts,
            &[("channel", channel)],
        );
        let draft = registry.draft(&request, &ledger)?;
        ledger.post(entry(1_000 + n)?, draft)?;
        requested_fees += merchant_fee + payer_fee;
        net[m] += amount - merchant_fee;
    }

    let fee_lines: i64 = ledger
        .history(fees)
        .iter()
        .map(|(_, line)| line.amount().amount())
        .sum();
    let fee_revenue = ledger.account(fees).map(|a| a.balance().amount());
    assert_eq!(fee_revenue, Some(fee_lines));
    assert_eq!(fee_lines, requested_fees);
    for (m, merchant) in merchants.iter().enumerate() {
        assert_eq!(
            ledger.account(*merchant).map(|a| a.balance().amount()),
            Some(net[m])
        );
    }
    mezgeb_core::check_invariants(&ledger).map_err(|v| v.to_string())?;
    Ok(())
}

/// 100 payers pay one merchant at the same moment, on PostgreSQL: every
/// payment posts, the merchant is owed exactly the sum of the net amounts,
/// and fee revenue holds exactly the sum of the fees.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn one_hundred_payers_one_merchant() -> TestResult {
    let Some(server) = support::Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let client: ClientId = support::uuid7(0xc11e).parse()?;
    let options = PostOptions {
        lock_timeout: std::time::Duration::from_secs(20),
        ..PostOptions::default()
    };
    let mut store = PgStore::with_options(db.app_pool_of(20).await?, client, options);
    // Templates draft against a mirror of the same accounts; the engine
    // checks every draft again when it posts it.
    let mut mirror = MemLedger::new();
    let chart = Chart::from_toml(CHART)?;
    let (bank, merchant, fees) = (id(1)?, id(2)?, id(3)?);
    for (account, reference) in [
        (bank, "1010"),
        (merchant, "2030:merch-coffee"),
        (fees, "4010"),
    ] {
        store.open_account(account, &chart, reference).await?;
        mirror.open_account(account, &chart, reference)?;
    }
    let mut payers = Vec::new();
    for n in 0..100u64 {
        let payer = id(100 + n)?;
        let reference = format!("2010:cust-{n}");
        store.open_account(payer, &chart, &reference).await?;
        mirror.open_account(payer, &chart, &reference)?;
        let deposit = EntryDraft::default()
            .debit(bank, etb(1_000_000))
            .credit(payer, etb(1_000_000));
        store.post(entry(n)?, deposit.clone()).await?;
        mirror.post(entry(n)?, deposit)?;
        payers.push(payer);
    }

    let registry = TemplateRegistry::standard();
    let mut drafts = Vec::new();
    let (mut expected_net, mut expected_fees) = (0, 0);
    for (n, payer) in payers.iter().enumerate() {
        let n = n as i64;
        let amount = 10_000 + 137 * n;
        let merchant_fee = amount / 50;
        let payer_fee = n % 3;
        let request = payment(
            "merchant_payment.v1",
            &[
                ("payer_wallet", *payer),
                ("merchant_balance", merchant),
                ("fee_revenue", fees),
            ],
            &[
                ("amount", etb(amount)),
                ("merchant_fee", etb(merchant_fee)),
                ("payer_fee", etb(payer_fee)),
            ],
            &[("channel", CHANNELS[n as usize % CHANNELS.len()])],
        );
        drafts.push((entry(1_000 + n as u64)?, registry.draft(&request, &mirror)?));
        expected_net += amount - merchant_fee;
        expected_fees += merchant_fee + payer_fee;
    }

    let tasks: Vec<_> = drafts
        .into_iter()
        .map(|(id, draft)| {
            let mut store = store.clone();
            tokio::spawn(async move { store.post(id, draft).await })
        })
        .collect();
    for task in tasks {
        task.await??;
    }

    let balance = |account| {
        let mut store = store.clone();
        async move {
            store
                .get_account(account)
                .await
                .map(|a| a.map(|a| a.balance().amount()))
        }
    };
    assert_eq!(balance(merchant).await?, Some(expected_net));
    assert_eq!(balance(fees).await?, Some(expected_fees));
    assert!(integrity_report(&mut db.app().await?).await?.is_clean());
    drop(store);
    db.drop().await?;
    Ok(())
}
