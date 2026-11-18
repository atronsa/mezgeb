//! Offline reconciliation on PostgreSQL (step 2.15, ADR-0020): a clean
//! offline life has no breaks; counters a device reported but nobody
//! uploaded are found once their voucher has ended; a device reporting less
//! than it signed is found at once.

mod offline_support;
mod support;

use mezgeb_core::Timestamp;
use mezgeb_engine::recon::{
    BreakFilter, BreakKind, BreakStatus, ReconBreak, list_breaks, reconcile_offline,
};
use mezgeb_offline::Outcome;
use offline_support::*;
use support::TestResult;

/// Tier A: 7 days to expiry, then 7 of grace.
const RETURN_AT: i64 = NOW + 14 * DAY + HOUR;

async fn open(conn: &mut sqlx::PgConnection) -> Result<Vec<ReconBreak>, BoxError> {
    Ok(list_breaks(
        conn,
        &BreakFilter {
            status: Some(BreakStatus::Open),
            limit: 100,
            ..BreakFilter::default()
        },
    )
    .await?)
}

#[tokio::test]
async fn reported_counters_never_received_are_found_once_the_voucher_ends() -> TestResult {
    let Some(w) = world(10_000, 200).await? else {
        return Ok(());
    };
    let mut admin = w.db.admin().await?;
    mezgeb_engine::set_late_deadline(&mut admin, w.client, 10).await?;
    drop(admin);
    let mut conn = w.db.app().await?;
    let mut verifier = w.db.verifier().await?;

    // Counters 1 and 2 settle, with a 2% fee; 3 stays with the payee.
    let chain = w.chain(&[(COFFEE, 1_000), (COFFEE, 1_000), (STALL, 1_000)])?;
    for r in &chain[..2] {
        assert_eq!(w.upload(&mut conn, r).await?.outcome, Outcome::Settled);
    }
    let run = reconcile_offline(&mut verifier, Timestamp::from_micros(NOW + DAY)).await?;
    assert_eq!(
        (run.breaks_new, run.breaks_open),
        (0, 0),
        "a clean life: {run:?}"
    );
    assert_eq!(run.items, 3, "one voucher and two settled receipts");

    // The voucher returns; the device reports 3 for its next voucher.
    w.tick(&mut conn, NOW + 7 * DAY).await?;
    w.tick(&mut conn, RETURN_AT).await?;
    let second = w
        .issue_after(&mut conn, 2, 1_000, RETURN_AT + DAY, 3)
        .await??;
    assert_eq!(second.voucher.counter_start, 4);
    // Counter 3 may still arrive late: no break yet.
    let run = reconcile_offline(&mut verifier, Timestamp::from_micros(RETURN_AT + DAY)).await?;
    assert_eq!(run.breaks_new, 0);

    // After the late deadline it can no longer be paid: a break.
    w.tick(&mut conn, RETURN_AT + 11 * DAY).await?;
    let run =
        reconcile_offline(&mut verifier, Timestamp::from_micros(RETURN_AT + 11 * DAY)).await?;
    assert_eq!(run.breaks_new, 1);
    let breaks = open(&mut verifier).await?;
    assert_eq!(breaks.len(), 1);
    assert_eq!(breaks[0].kind, BreakKind::MissingReceipts);
    assert_eq!(
        breaks[0].voucher,
        Some(w.voucher.voucher.voucher_id.as_uuid())
    );
    assert!(
        breaks[0].detail.contains("1 to 3; 1 of them"),
        "{}",
        breaks[0].detail
    );

    // The payee finally uploads it: refused as too late, but now known.
    let late = w
        .upload_at(&mut conn, &chain[2], RETURN_AT + 12 * DAY)
        .await?;
    assert_ne!(late.outcome, Outcome::Settled);
    let run =
        reconcile_offline(&mut verifier, Timestamp::from_micros(RETURN_AT + 12 * DAY)).await?;
    assert_eq!((run.breaks_cleared, run.breaks_open), (1, 0));

    drop((conn, verifier));
    w.db.drop().await?;
    Ok(())
}

#[tokio::test]
async fn a_device_reporting_less_than_it_signed_is_found() -> TestResult {
    let Some(w) = world(10_000, 0).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let chain = w.chain(&[(COFFEE, 1_000), (COFFEE, 1_000)])?;
    for r in &chain {
        assert_eq!(w.upload(&mut conn, r).await?.outcome, Outcome::Settled);
    }
    w.tick(&mut conn, NOW + 7 * DAY).await?;
    w.tick(&mut conn, RETURN_AT).await?;
    // It says its last counter is 1, but counter 2 was received.
    let second = w
        .issue_after(&mut conn, 2, 1_000, RETURN_AT + DAY, 1)
        .await??;
    assert_eq!(second.voucher.counter_start, 3, "the ledger's record wins");

    let run = reconcile_offline(&mut conn, Timestamp::from_micros(RETURN_AT + DAY)).await?;
    assert_eq!(run.breaks_new, 1);
    let breaks = open(&mut conn).await?;
    assert_eq!(breaks[0].kind, BreakKind::CounterBehind);
    assert_eq!(breaks[0].voucher, Some(second.voucher.voucher_id.as_uuid()));
    assert_eq!(breaks[0].device, Some(w.device.as_uuid()));
    // Found again, raised once.
    let again = reconcile_offline(&mut conn, Timestamp::from_micros(RETURN_AT + DAY)).await?;
    assert_eq!((again.breaks_new, again.breaks_open), (0, 1));

    drop(conn);
    w.db.drop().await?;
    Ok(())
}
