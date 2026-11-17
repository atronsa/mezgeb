//! The exit gate of step 2.13: the ten adversarial scenarios of
//! `docs/offline/scenarios.md`, each as an integration test on PostgreSQL,
//! with its outcomes and its accounting result.
//!
//! The setup follows the walkthroughs, with one difference: every payer
//! device here is a card of tier A, so a voucher expires after 7 days and
//! returns after 7 more of grace (the walkthroughs' phone, tier B, expires
//! after 3). The merchant fee is 2%. Each test ends by checking that every
//! escrow holds its voucher's unspent value, that the hash chains verify
//! (INV-11), and, as every database test does, that the balances verify.

mod offline_support;
mod support;

use mezgeb_core::Timestamp;
use mezgeb_engine::{
    DeviceStatus, DisputeResolution, OfflineError, PostOptions, disputes, entry_record,
    escrow_findings, get_device, resolve_dispute, set_device_status, shortfalls, verify_chains,
    voucher_receipts,
};
use mezgeb_offline::{Outcome, Receipt};
use offline_support::*;
use support::TestResult;

const RETURN_AT: i64 = NOW + 14 * DAY + HOUR;

async fn finish(w: World) -> TestResult {
    let mut conn = w.db.app().await?;
    assert_eq!(escrow_findings(&mut conn).await?, vec![]);
    let mut verifier = w.db.verifier().await?;
    let chains = verify_chains(&mut verifier, &[]).await?;
    assert!(chains.is_clean(), "{:?}", chains.findings);
    drop((conn, verifier));
    w.db.drop().await
}

async fn outcomes(w: &World, conn: &mut sqlx::PgConnection) -> Result<Vec<Outcome>, BoxError> {
    Ok(
        voucher_receipts(conn, w.client, w.voucher.voucher.voucher_id)
            .await?
            .iter()
            .map(|r| r.outcome)
            .collect(),
    )
}

/// 1. Clone: a copy of the phone spends counter 4 again.
#[tokio::test]
async fn scenario_1_clone() -> TestResult {
    let Some(w) = world(10_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    // Counters 1–3 spent 40; 60 left when the phone is copied.
    let real = w.chain(&[
        (COFFEE, 1_000),
        (COFFEE, 1_000),
        (COFFEE, 2_000),
        (COFFEE, 5_000),
    ])?;
    let copy = w.chain(&[
        (COFFEE, 1_000),
        (COFFEE, 1_000),
        (COFFEE, 2_000),
        (STALL, 4_500),
    ])?;
    for r in &real {
        assert_eq!(w.upload(&mut conn, r).await?.outcome, Outcome::Settled);
    }
    assert_eq!(
        w.upload(&mut conn, &copy[3]).await?.outcome,
        Outcome::Conflict
    );
    // A later receipt of the copy: kept for the dispute.
    let mut later = Receipt::decode(&copy[3].0)?;
    later.counter = 5;
    later.prev_digest = Receipt::decode(&copy[3].0)?.digest();
    later.amount = etb(500);
    assert_eq!(
        w.upload(&mut conn, &w.signed(&later)).await?.outcome,
        Outcome::Frozen
    );

    // The dispute finds both validly signed: paid.
    let wallet = balance(&mut conn, id(HANA)?).await?;
    let dispute = disputes(&mut conn, None, true).await?.remove(0);
    resolve_dispute(
        &mut conn,
        dispute.id,
        DisputeResolution::Paid,
        "operator:review",
        Timestamp::from_micros(NOW + DAY),
        accounts(),
        &TestBooks,
        &PostOptions::default(),
    )
    .await?;
    // Stall's 45 from Hana's wallet (E31); counter 5's 5 from the escrow
    // left; the 5 left after it returns (E27).
    assert_eq!(balance(&mut conn, id(STALL)?).await?, 4_410 + 490);
    assert_eq!(balance(&mut conn, id(HANA)?).await?, wallet - 4_500 + 500);
    assert_eq!(
        balance(&mut conn, w.voucher.voucher.escrow_account_id).await?,
        0
    );
    assert_eq!(balance(&mut conn, id(LOSS)?).await?, 0);
    assert_eq!(
        get_device(&mut conn, w.client, w.device).await?.status,
        DeviceStatus::Revoked
    );
    drop(conn);
    finish(w).await
}

/// 2. Replay: Coffee uploads one receipt three times, and gives a copy to
/// Stall, which uploads it too.
#[tokio::test]
async fn scenario_2_replay() -> TestResult {
    let Some(w) = world(10_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let r = w.chain(&[(COFFEE, 3_000)])?;
    let mut seen = Vec::new();
    for _ in 0..4 {
        seen.push(w.upload(&mut conn, &r[0]).await?.outcome);
    }
    assert_eq!(
        seen,
        [
            Outcome::Settled,
            Outcome::Duplicate,
            Outcome::Duplicate,
            Outcome::Duplicate
        ]
    );
    assert_eq!(balance(&mut conn, id(COFFEE)?).await?, 2_940);
    assert_eq!(balance(&mut conn, id(FEES)?).await?, 60);
    assert_eq!(balance(&mut conn, id(STALL)?).await?, 0);
    drop(conn);
    finish(w).await
}

/// 3. Forged amount: Stall edits 20 to 200.
#[tokio::test]
async fn scenario_3_forged_amount() -> TestResult {
    let Some(w) = world(10_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let r = w.chain(&[(COFFEE, 1_000), (STALL, 2_000)])?;
    w.upload(&mut conn, &r[0]).await?;
    let mut edited = Receipt::decode(&r[1].0)?;
    edited.amount = etb(20_000);
    let forged = (edited.body(), r[1].1.clone());
    assert_eq!(
        w.upload(&mut conn, &forged).await?.outcome,
        Outcome::BadSignature
    );
    assert_eq!(balance(&mut conn, id(STALL)?).await?, 0);
    assert_eq!(w.upload(&mut conn, &r[1]).await?.outcome, Outcome::Settled);
    assert_eq!(balance(&mut conn, id(STALL)?).await?, 1_960);
    drop(conn);
    finish(w).await
}

/// 4. Clock rollback: the phone's clock says a day long past; ledger time
/// decides.
#[tokio::test]
async fn scenario_4_clock_rollback() -> TestResult {
    for upload_at in [NOW + 9 * DAY, RETURN_AT + DAY] {
        let Some(w) = world(10_000, 200).await? else {
            return Ok(());
        };
        let mut conn = w.db.app().await?;
        let r = w.chain(&[(COFFEE, 1_000), (COFFEE, 1_000), (COFFEE, 4_000)])?;
        w.upload(&mut conn, &r[0]).await?;
        w.upload(&mut conn, &r[1]).await?;
        // Made on day 8, in grace, with the clock reading day 2.
        let mut rolled = Receipt::decode(&r[2].0)?;
        rolled.event_time = Timestamp::from_micros(NOW + 2 * DAY);
        let rolled = w.signed(&rolled);
        w.tick(&mut conn, NOW + 7 * DAY).await?;
        if upload_at > RETURN_AT {
            w.tick(&mut conn, RETURN_AT).await?;
        }
        let result = w.upload_at(&mut conn, &rolled, upload_at).await?;
        let entry = entry_record(&mut conn, result.entry.ok_or("not paid")?)
            .await?
            .ok_or("no entry")?;
        // Day 2 lies between the voucher's issue and now: recorded.
        assert_eq!(
            entry.entry.event_time(),
            Some(Timestamp::from_micros(NOW + 2 * DAY))
        );
        if upload_at < RETURN_AT {
            // In grace by ledger time: from escrow.
            assert_eq!(result.outcome, Outcome::Settled);
        } else {
            // After the return: from Hana's wallet (E28).
            assert_eq!(result.outcome, Outcome::SettledLate);
        }
        assert_eq!(balance(&mut conn, id(COFFEE)?).await?, 5_880);
        drop(conn);
        finish(w).await?;
    }
    Ok(())
}

/// 5. Payee double-upload: Coffee's batch, its retry, then Hana's phone.
#[tokio::test]
async fn scenario_5_payee_double_upload() -> TestResult {
    let Some(w) = world(10_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let r = w.chain(&[(COFFEE, 1_000), (COFFEE, 1_000)])?;
    let mut seen = Vec::new();
    for _ in 0..3 {
        for receipt in &r {
            seen.push(w.upload(&mut conn, receipt).await?.outcome);
        }
    }
    assert_eq!(&seen[..2], [Outcome::Settled; 2]);
    assert_eq!(&seen[2..], [Outcome::Duplicate; 4]);
    assert_eq!(balance(&mut conn, id(COFFEE)?).await?, 1_960);
    assert_eq!(balance(&mut conn, id(FEES)?).await?, 40);
    drop(conn);
    finish(w).await
}

/// 6. Payer never syncs: only the merchants upload, out of order; the
/// rest returns at the end.
#[tokio::test]
async fn scenario_6_payer_never_syncs() -> TestResult {
    let Some(w) = world(50_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let wallet = balance(&mut conn, id(HANA)?).await?;
    let r = w.chain(&[(COFFEE, 12_000), (STALL, 8_000)])?;
    assert_eq!(
        w.upload_at(&mut conn, &r[1], NOW + 2 * DAY).await?.outcome,
        Outcome::WaitingForGap
    );
    let coffee = w.upload_at(&mut conn, &r[0], NOW + 4 * DAY).await?;
    assert_eq!(coffee.outcome, Outcome::Settled);
    assert_eq!(coffee.settled.len(), 2);
    w.tick(&mut conn, NOW + 7 * DAY).await?;
    let report = w.tick(&mut conn, RETURN_AT).await?;
    assert_eq!(report.returned.len(), 1);
    // 500 = 120 + 80 + 300 returned (E18).
    assert_eq!(balance(&mut conn, id(HANA)?).await?, wallet + 30_000);
    assert_eq!(
        balance(&mut conn, w.voucher.voucher.escrow_account_id).await?,
        0
    );
    drop(conn);
    finish(w).await
}

/// 7. Lost device: reported lost, a finder spends what is left in escrow;
/// the payee is paid, and the device gets no new voucher.
#[tokio::test]
async fn scenario_7_lost_device() -> TestResult {
    let Some(w) = world(10_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let r = w.chain(&[(COFFEE, 3_000), (STALL, 3_000)])?;
    w.upload(&mut conn, &r[0]).await?;
    set_device_status(
        &mut conn,
        Some(w.client),
        w.device,
        DeviceStatus::Lost,
        Some("owner report"),
    )
    .await?;
    assert!(matches!(
        w.issue_again(&mut conn, 2, 1_000, NOW + DAY).await?,
        Err(OfflineError::DeviceNotActive(DeviceStatus::Lost))
    ));
    // The finder's payment, accepted in good faith.
    assert_eq!(w.upload(&mut conn, &r[1]).await?.outcome, Outcome::Settled);
    let wallet = balance(&mut conn, id(HANA)?).await?;
    w.tick(&mut conn, NOW + 7 * DAY).await?;
    w.tick(&mut conn, RETURN_AT).await?;
    // The 40 left returns; Hana lost at most the escrow.
    assert_eq!(balance(&mut conn, id(HANA)?).await?, wallet + 4_000);
    assert_eq!(balance(&mut conn, id(STALL)?).await?, 2_940);
    drop(conn);
    finish(w).await
}

/// 8. Partial sync: half the receipts, then the payees, out of order; and
/// the gap never filled, settled at the timeout.
#[tokio::test]
async fn scenario_8_partial_sync() -> TestResult {
    let payments = [
        (COFFEE, 1_500),
        (COFFEE, 1_500),
        (COFFEE, 1_500),
        (STALL, 1_500),
        (COFFEE, 1_500),
        (STALL, 1_500),
    ];
    // Every receipt arrives in the end.
    let Some(w) = world(10_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let r = w.chain(&payments)?;
    for receipt in &r[..3] {
        assert_eq!(
            w.upload(&mut conn, receipt).await?.outcome,
            Outcome::Settled
        );
    }
    assert_eq!(
        w.upload(&mut conn, &r[4]).await?.outcome,
        Outcome::WaitingForGap
    );
    let four = w.upload(&mut conn, &r[3]).await?;
    assert_eq!((four.outcome, four.settled.len()), (Outcome::Settled, 2));
    assert_eq!(w.upload(&mut conn, &r[5]).await?.outcome, Outcome::Settled);
    assert_eq!(
        balance(&mut conn, w.voucher.voucher.escrow_account_id).await?,
        1_000
    );
    drop(conn);
    finish(w).await?;

    // Stall never uploads receipt 4 in time: 5 and 6 settle past the gap
    // at 72 hours; 4, arriving later, matches 5's link and settles.
    let Some(w) = world(10_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let r = w.chain(&payments)?;
    for i in [0, 1, 2, 4, 5] {
        w.upload(&mut conn, &r[i]).await?;
    }
    let report = w.tick(&mut conn, UPLOADED + 73 * HOUR).await?;
    assert_eq!(report.settled_past_gaps[0].1.len(), 2);
    assert_eq!(w.upload(&mut conn, &r[3]).await?.outcome, Outcome::Settled);
    assert_eq!(outcomes(&w, &mut conn).await?, [Outcome::Settled; 6]);
    assert_eq!(
        balance(&mut conn, w.voucher.voucher.escrow_account_id).await?,
        1_000
    );
    drop(conn);
    finish(w).await
}

/// 9. Voucher expiry race: made a minute before expiry, uploaded 20 days
/// later, the wallet holding 25 of 90: E29, a shortfall, and no voucher
/// until it is repaid; after the late deadline, EXPIRED.
#[tokio::test]
async fn scenario_9_voucher_expiry_race() -> TestResult {
    let Some(w) = world(50_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let mut receipt = Receipt::decode(&w.chain(&[(STALL, 9_000)])?[0].0)?;
    receipt.event_time = Timestamp::from_micros(NOW + 7 * DAY - 60_000_000);
    let receipt = w.signed(&receipt);
    w.tick(&mut conn, NOW + 7 * DAY).await?;
    w.tick(&mut conn, RETURN_AT).await?;
    w.spend_wallet_to(2_500).await?;
    let late = w.upload_at(&mut conn, &receipt, NOW + 27 * DAY).await?;
    assert_eq!(late.outcome, Outcome::SettledLate);
    assert_eq!(balance(&mut conn, id(HANA)?).await?, 0);
    assert_eq!(balance(&mut conn, id(LOSS)?).await?, 6_500);
    assert_eq!(balance(&mut conn, id(STALL)?).await?, 8_820);
    let owed = shortfalls(&mut conn, w.client, None).await?;
    assert_eq!(owed[0].amount, etb(6_500));
    assert!(matches!(
        w.issue_again(&mut conn, 2, 1_000, NOW + 28 * DAY).await?,
        Err(OfflineError::ShortfallUnpaid { .. })
    ));
    drop(conn);
    finish(w).await?;

    // Uploaded after the late deadline: refused, nothing posted.
    let Some(w) = world(50_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let receipt = w.chain(&[(STALL, 9_000)])?.remove(0);
    w.tick(&mut conn, NOW + 7 * DAY).await?;
    w.tick(&mut conn, RETURN_AT).await?;
    w.tick(&mut conn, RETURN_AT + 91 * DAY).await?;
    let expired = w
        .upload_at(&mut conn, &receipt, RETURN_AT + 92 * DAY)
        .await?;
    assert_eq!(expired.outcome, Outcome::Expired);
    assert_eq!(balance(&mut conn, id(STALL)?).await?, 0);
    drop(conn);
    finish(w).await
}

/// 10. Colluding merchant: Stall holds the whole voucher's receipt until
/// the escrow went back and Hana emptied her wallet.
#[tokio::test]
async fn scenario_10_colluding_merchant() -> TestResult {
    let Some(w) = world(10_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let r = w.chain(&[(STALL, 10_000)])?;
    w.tick(&mut conn, NOW + 7 * DAY).await?;
    w.tick(&mut conn, RETURN_AT).await?;
    w.spend_wallet_to(0).await?;
    let late = w.upload_at(&mut conn, &r[0], RETURN_AT + 10 * DAY).await?;
    assert_eq!(late.outcome, Outcome::SettledLate);
    // E29: nothing from the wallet, 100 from 5030, recorded against Hana.
    assert_eq!(balance(&mut conn, id(LOSS)?).await?, 10_000);
    assert_eq!(
        shortfalls(&mut conn, w.client, None).await?[0].amount,
        etb(10_000)
    );
    assert!(matches!(
        w.issue_again(&mut conn, 2, 1_000, RETURN_AT + 11 * DAY)
            .await?,
        Err(OfflineError::ShortfallUnpaid { .. })
    ));
    // The other variants fail: twice is a duplicate, an invented receipt a
    // bad signature.
    assert_eq!(
        w.upload_at(&mut conn, &r[0], RETURN_AT + 11 * DAY)
            .await?
            .outcome,
        Outcome::Duplicate
    );
    let mut invented = Receipt::decode(&r[0].0)?;
    invented.counter = 2;
    invented.prev_digest = Receipt::decode(&r[0].0)?.digest();
    let invented = (
        invented.body(),
        invented.sign(&key("not hana's device")?).to_vec(),
    );
    assert_eq!(
        w.upload_at(&mut conn, &invented, RETURN_AT + 11 * DAY)
            .await?
            .outcome,
        Outcome::BadSignature
    );
    assert_eq!(balance(&mut conn, id(LOSS)?).await?, 10_000);
    drop(conn);
    finish(w).await
}
