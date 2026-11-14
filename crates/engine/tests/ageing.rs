//! The ageing query on PostgreSQL (step 2.02).

#[path = "../../core/tests/common/mod.rs"]
mod common;
mod support;

use mezgeb_core::{
    AccountId, Chart, ClientId, Currency, EntryDraft, EntryId, Ledger, Money, Timestamp,
};
use mezgeb_engine::{PgStore, ageing};
use support::{Server, TestResult, uuid7};

const DAY: i64 = 86_400_000_000;

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

fn entry(n: u64) -> Result<EntryId, mezgeb_core::IdError> {
    uuid7(1_000 + n).parse()
}

/// Open items are the accounts under the given codes whose balance is not
/// zero, oldest first, with their age; closed items and other codes are
/// left out.
#[tokio::test]
async fn ageing_lists_open_items_oldest_first() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let client: ClientId = uuid7(0xc11e).parse()?;
    let mut store = PgStore::new(db.app_pool().await?, client);
    let chart = Chart::from_toml(common::CHART)?;
    let mut open = Vec::new();
    for (n, reference) in [
        "1010",
        "2010:cust-a",
        "2020:wd-1",
        "2020:wd-2",
        "9030:wdt-2",
        "9010:sus-1",
    ]
    .into_iter()
    .enumerate()
    {
        let id: AccountId = uuid7(n as u64).parse()?;
        store.open_account(id, &chart, reference).await?;
        open.push(id);
    }
    let [bank, wallet, wd1, wd2, wdt2, sus1] = open[..] else {
        return Err("six accounts".into());
    };
    let mut n = 0;
    let mut post = async |draft: EntryDraft| -> TestResult {
        n += 1;
        store.post(entry(n)?, draft).await?;
        Ok(())
    };

    post(
        EntryDraft::default()
            .debit(bank, etb(100_000))
            .credit(wallet, etb(100_000)),
    )
    .await?;
    // wd-1 is requested and stays pending.
    post(
        EntryDraft::default()
            .debit(wallet, etb(30_000))
            .credit(wd1, etb(30_000)),
    )
    .await?;
    // An unidentified deposit.
    post(
        EntryDraft::default()
            .debit(bank, etb(5_000))
            .credit(sus1, etb(5_000)),
    )
    .await?;
    // wd-2 is requested and sent: wd-2 is back at zero, wdt-2 is in transit.
    post(
        EntryDraft::default()
            .debit(wallet, etb(20_000))
            .credit(wd2, etb(20_000)),
    )
    .await?;
    post(
        EntryDraft::default()
            .debit(wd2, etb(20_000))
            .credit(wdt2, etb(20_000)),
    )
    .await?;

    let codes = [2020, 9010, 9030];
    let mut conn = db.app().await?;
    let first = ageing(&mut conn, &codes, Timestamp::from_micros(0)).await?;
    let opened: Vec<AccountId> = first.iter().map(|i| i.account.id()).collect();
    assert_eq!(opened, [wd1, sus1, wdt2], "open items, oldest first");
    let balances: Vec<i64> = first.iter().map(|i| i.account.balance().amount()).collect();
    assert_eq!(balances, [30_000, 5_000, 20_000]);
    assert!(first.windows(2).all(|w| w[0].since <= w[1].since));
    // A clock behind the ledger gives age zero, never a negative age.
    assert!(first.iter().all(|i| i.age_micros == 0));

    // Three days after wd-1 opened.
    let now = Timestamp::from_micros(first[0].since.as_micros() + 3 * DAY);
    let later = ageing(&mut conn, &codes, now).await?;
    assert_eq!(later[0].age_micros, 3 * DAY);
    for item in &later {
        assert_eq!(item.age_micros, now.as_micros() - item.since.as_micros());
    }

    // Only the codes asked for.
    let suspense = ageing(&mut conn, &[9010], now).await?;
    assert_eq!(
        suspense.iter().map(|i| i.account.id()).collect::<Vec<_>>(),
        [sus1]
    );
    assert!(ageing(&mut conn, &[], now).await?.is_empty());

    // Once an item is back at zero, it is no longer listed.
    drop(conn);
    post(
        EntryDraft::default()
            .debit(wdt2, etb(20_000))
            .credit(bank, etb(20_000)),
    )
    .await?;
    let mut conn = db.app().await?;
    let after = ageing(&mut conn, &codes, now).await?;
    assert_eq!(
        after.iter().map(|i| i.account.id()).collect::<Vec<_>>(),
        [wd1, sus1]
    );
    drop(conn);
    db.drop().await?;
    Ok(())
}
