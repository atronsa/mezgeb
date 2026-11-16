//! A voucher's whole life on PostgreSQL (step 2.13): the gap timeout,
//! expiry, grace and return, late receipts by the loss policy, shortfalls
//! and their repayment, the late deadline, disputes, and an expiry racing a
//! receipt.

mod offline_support;
mod support;

use std::sync::Arc;

use mezgeb_core::Timestamp;
use mezgeb_engine::{
    DeviceStatus, DisputeResolution, OfflineError, PostOptions, disputes, escrow_findings,
    get_device, get_voucher, repay_shortfall, resolve_dispute, shortfalls, voucher_receipts,
};
use mezgeb_offline::{Outcome, Receipt};
use offline_support::*;
use support::TestResult;

async fn repay(
    conn: &mut sqlx::PgConnection,
    client: mezgeb_core::ClientId,
    id: uuid::Uuid,
) -> Result<mezgeb_engine::Shortfall, OfflineError> {
    repay_shortfall(
        conn,
        client,
        id,
        accounts(),
        &TestBooks,
        &PostOptions::default(),
    )
    .await
}

/// Tier A: 7 days to expiry, then 7 of grace.
const RETURN_AT: i64 = NOW + 14 * DAY + HOUR;

async fn outcome_of(
    w: &World,
    conn: &mut sqlx::PgConnection,
    counter: u64,
) -> Result<Outcome, BoxError> {
    let all = voucher_receipts(conn, w.client, w.voucher.voucher.voucher_id).await?;
    Ok(all
        .iter()
        .find(|r| r.counter == counter)
        .ok_or("no such receipt")?
        .outcome)
}

/// A gap waits 72 hours, then the waiting receipts settle past it; the
/// missing receipt, arriving later, must be the one its successor named.
#[tokio::test]
async fn a_gap_times_out_and_the_missing_receipt_must_match() -> TestResult {
    let Some(w) = world(10_000, 0).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let chain = w.chain(&[(COFFEE, 1_000), (STALL, 2_000), (COFFEE, 3_000)])?;
    assert_eq!(
        w.upload(&mut conn, &chain[1]).await?.outcome,
        Outcome::WaitingForGap
    );
    assert_eq!(
        w.upload(&mut conn, &chain[2]).await?.outcome,
        Outcome::WaitingForGap
    );

    // Not yet: 71 hours.
    let report = w.tick(&mut conn, wall() + 71 * HOUR).await?;
    assert!(report.settled_past_gaps.is_empty());
    // 73 hours: both settle, in order, past the gap.
    let report = w.tick(&mut conn, wall() + 73 * HOUR).await?;
    assert_eq!(report.settled_past_gaps.len(), 1);
    assert_eq!(report.settled_past_gaps[0].1.len(), 2);
    assert_eq!(outcome_of(&w, &mut conn, 2).await?, Outcome::Settled);
    assert_eq!(outcome_of(&w, &mut conn, 3).await?, Outcome::Settled);
    let skipped: bool =
        sqlx::query_scalar("SELECT gap_skipped FROM offline_receipts WHERE counter = 2")
            .fetch_one(&mut conn)
            .await?;
    assert!(skipped);

    // A different receipt 1 is not the one receipt 2 committed to.
    let mut other = Receipt::decode(&chain[0].0)?;
    other.amount = etb(999);
    assert_eq!(
        w.upload(&mut conn, &w.signed(&other)).await?.outcome,
        Outcome::Conflict
    );
    drop(conn);
    w.db.drop().await?;

    // The genuine one settles from the escrow left.
    let Some(w) = world(10_000, 0).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let chain = w.chain(&[(COFFEE, 1_000), (STALL, 2_000)])?;
    w.upload(&mut conn, &chain[1]).await?;
    w.tick(&mut conn, wall() + 73 * HOUR).await?;
    assert_eq!(
        w.upload(&mut conn, &chain[0]).await?.outcome,
        Outcome::Settled
    );
    assert_eq!(
        balance(&mut conn, w.voucher.voucher.escrow_account_id).await?,
        7_000
    );
    assert_eq!(escrow_findings(&mut conn).await?, vec![]);
    drop(conn);
    w.db.drop().await?;
    Ok(())
}

/// Expiry, grace, the return of the escrow left (E18), a late receipt from
/// the wallet (E28), one the wallet cannot cover (E29) with its shortfall,
/// the block on new vouchers, the repayment (E30), and the late deadline.
#[tokio::test]
async fn a_voucher_expires_returns_and_ends() -> TestResult {
    let Some(w) = world(10_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let chain = w.chain(&[(COFFEE, 2_000), (STALL, 4_000), (COFFEE, 3_000)])?;
    w.upload(&mut conn, &chain[0]).await?;
    let wallet_before = balance(&mut conn, id(HANA)?).await?;

    let report = w.tick(&mut conn, NOW + 7 * DAY).await?;
    assert_eq!(report.graced, vec![w.voucher.voucher.voucher_id]);
    // In grace, a receipt still settles from escrow.
    assert_eq!(
        w.upload_at(&mut conn, &chain[1], NOW + 8 * DAY)
            .await?
            .outcome,
        Outcome::Settled
    );
    let report = w.tick(&mut conn, RETURN_AT).await?;
    assert_eq!(report.returned.len(), 1);
    let voucher = get_voucher(&mut conn, w.client, w.voucher.voucher.voucher_id).await?;
    assert_eq!(voucher.status.as_str(), "returned");
    // E18: the 40 ETB left went back.
    assert_eq!(
        balance(&mut conn, voucher.voucher.escrow_account_id).await?,
        0
    );
    assert_eq!(balance(&mut conn, id(HANA)?).await?, wallet_before + 4_000);

    // A late receipt, the wallet able to pay: E28.
    let late = w.upload_at(&mut conn, &chain[2], RETURN_AT + DAY).await?;
    assert_eq!(late.outcome, Outcome::SettledLate);
    assert_eq!(balance(&mut conn, id(HANA)?).await?, wallet_before + 1_000);
    assert!(shortfalls(&mut conn, w.client, None).await?.is_empty());
    drop(conn);
    w.db.drop().await?;

    // The wallet holds 25 ETB of a 90 ETB late receipt: E29.
    let Some(w) = world(10_000, 200).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let chain = w.chain(&[(STALL, 9_000)])?;
    w.tick(&mut conn, NOW + 7 * DAY).await?;
    w.tick(&mut conn, RETURN_AT).await?;
    w.spend_wallet_to(2_500).await?;
    let late = w
        .upload_at(&mut conn, &chain[0], RETURN_AT + 6 * DAY)
        .await?;
    assert_eq!(late.outcome, Outcome::SettledLate);
    assert_eq!(balance(&mut conn, id(HANA)?).await?, 0);
    assert_eq!(balance(&mut conn, id(LOSS)?).await?, 6_500);
    assert_eq!(balance(&mut conn, id(STALL)?).await?, 8_820);
    let owed = shortfalls(&mut conn, w.client, None).await?;
    assert_eq!(owed.len(), 1);
    assert_eq!(owed[0].amount, etb(6_500));

    // No new voucher until it is repaid.
    assert!(matches!(
        w.issue_again(&mut conn, 2, 1_000, RETURN_AT + 7 * DAY)
            .await?,
        Err(OfflineError::ShortfallUnpaid {
            amount_minor: 6_500
        })
    ));
    // Repaying needs the money (INV-8).
    assert!(repay(&mut conn, w.client, owed[0].id).await.is_err());
    let mut store = mezgeb_engine::PgStore::new(w.db.app_pool().await?, w.client);
    mezgeb_core::Ledger::post(
        &mut store,
        id(81_000)?,
        mezgeb_core::EntryDraft::default()
            .debit(id(BANK)?, etb(10_000))
            .credit(id(HANA)?, etb(10_000)),
    )
    .await?;
    let repaid = repay(&mut conn, w.client, owed[0].id).await?;
    assert!(repaid.repaid_entry.is_some());
    assert_eq!(balance(&mut conn, id(LOSS)?).await?, 0);
    // Repaying again changes nothing.
    assert_eq!(
        repay(&mut conn, w.client, owed[0].id).await?.repaid_entry,
        repaid.repaid_entry
    );
    w.issue_again(&mut conn, 2, 1_000, RETURN_AT + 7 * DAY)
        .await??;

    // After the late deadline (90 days), late receipts are EXPIRED.
    let report = w.tick(&mut conn, RETURN_AT + 91 * DAY).await?;
    assert_eq!(report.ended, vec![w.voucher.voucher.voucher_id]);
    let Some(w2) = world(10_000, 0).await? else {
        return Ok(());
    };
    let mut conn2 = w2.db.app().await?;
    let chain2 = w2.chain(&[(STALL, 1_000)])?;
    w2.tick(&mut conn2, NOW + 7 * DAY).await?;
    w2.tick(&mut conn2, RETURN_AT).await?;
    w2.tick(&mut conn2, RETURN_AT + 91 * DAY).await?;
    let expired = w2
        .upload_at(&mut conn2, &chain2[0], RETURN_AT + 92 * DAY)
        .await?;
    assert_eq!(expired.outcome, Outcome::Expired);
    assert_eq!(expired.entry, None);
    assert_eq!(balance(&mut conn2, id(STALL)?).await?, 0);
    assert_eq!(escrow_findings(&mut conn).await?, vec![]);
    drop((conn, conn2, store));
    w.db.drop().await?;
    w2.db.drop().await?;
    Ok(())
}

/// A clone's dispute, paid: the second receipt of the counter is paid from
/// the wallet (E31), the escrow left returns (E27), the device is revoked.
#[tokio::test]
async fn a_disputed_clone_is_paid_from_the_wallet() -> TestResult {
    let Some(w) = world(10_000, 0).await? else {
        return Ok(());
    };
    let mut conn = w.db.app().await?;
    let real = w.chain(&[(COFFEE, 5_000)])?;
    let clone = w.chain(&[(STALL, 4_500)])?;
    w.upload(&mut conn, &real[0]).await?;
    assert_eq!(
        w.upload(&mut conn, &clone[0]).await?.outcome,
        Outcome::Conflict
    );
    let wallet = balance(&mut conn, id(HANA)?).await?;

    // Frozen: time does not move it.
    w.tick(&mut conn, RETURN_AT).await?;
    assert_eq!(
        balance(&mut conn, w.voucher.voucher.escrow_account_id).await?,
        5_000
    );

    let open = disputes(&mut conn, None, true).await?;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].reason, "conflict");
    let resolved = resolve_dispute(
        &mut conn,
        open[0].id,
        DisputeResolution::Paid,
        "operator:abebe",
        Timestamp::from_micros(NOW + DAY),
        accounts(),
        &TestBooks,
        &PostOptions::default(),
    )
    .await?;
    assert_eq!(resolved.resolution, Some(DisputeResolution::Paid));
    // E31 from the wallet, E27 the escrow left.
    assert_eq!(balance(&mut conn, id(STALL)?).await?, 4_500);
    assert_eq!(balance(&mut conn, id(HANA)?).await?, wallet - 4_500 + 5_000);
    assert_eq!(
        balance(&mut conn, w.voucher.voucher.escrow_account_id).await?,
        0
    );
    assert_eq!(
        get_device(&mut conn, w.client, w.device).await?.status,
        DeviceStatus::Revoked
    );
    // Again, the same way: unchanged; the other way: refused.
    let again = resolve_dispute(
        &mut conn,
        open[0].id,
        DisputeResolution::Paid,
        "operator:abebe",
        Timestamp::from_micros(NOW + DAY),
        accounts(),
        &TestBooks,
        &PostOptions::default(),
    )
    .await?;
    assert_eq!(again, resolved);
    assert!(matches!(
        resolve_dispute(
            &mut conn,
            open[0].id,
            DisputeResolution::Refused,
            "operator:abebe",
            Timestamp::from_micros(NOW + DAY),
            accounts(),
            &TestBooks,
            &PostOptions::default(),
        )
        .await,
        Err(OfflineError::DisputeResolved("paid"))
    ));
    assert_eq!(escrow_findings(&mut conn).await?, vec![]);
    drop(conn);
    w.db.drop().await?;
    Ok(())
}

/// An over-spend's dispute, paid: in counter order up to the escrow, the
/// crossing receipt partly from escrow and partly from the wallet (E26),
/// the frozen ones after it from the wallet. Refused instead: nothing paid.
#[tokio::test]
async fn an_overspend_dispute_pays_in_order_or_refuses() -> TestResult {
    for resolution in [DisputeResolution::Paid, DisputeResolution::Refused] {
        let Some(w) = world(10_000, 0).await? else {
            return Ok(());
        };
        let mut conn = w.db.app().await?;
        let chain = w.chain(&[(COFFEE, 7_000), (STALL, 5_000), (COFFEE, 1_000)])?;
        for receipt in &chain {
            w.upload(&mut conn, receipt).await?;
        }
        assert_eq!(outcome_of(&w, &mut conn, 2).await?, Outcome::OverLimit);
        let wallet = balance(&mut conn, id(HANA)?).await?;
        let dispute = disputes(&mut conn, Some(w.client), true).await?.remove(0);
        assert_eq!(dispute.reason, "over_limit");
        resolve_dispute(
            &mut conn,
            dispute.id,
            resolution,
            "operator:abebe",
            Timestamp::from_micros(NOW + DAY),
            accounts(),
            &TestBooks,
            &PostOptions::default(),
        )
        .await?;
        match resolution {
            DisputeResolution::Paid => {
                // 2: 30 from escrow, 20 from the wallet; 3: 10 from the wallet.
                assert_eq!(outcome_of(&w, &mut conn, 2).await?, Outcome::SettledLate);
                assert_eq!(outcome_of(&w, &mut conn, 3).await?, Outcome::SettledLate);
                assert_eq!(balance(&mut conn, id(STALL)?).await?, 5_000);
                assert_eq!(balance(&mut conn, id(HANA)?).await?, wallet - 3_000);
            }
            DisputeResolution::Refused => {
                assert_eq!(outcome_of(&w, &mut conn, 2).await?, Outcome::Refused);
                assert_eq!(outcome_of(&w, &mut conn, 3).await?, Outcome::Refused);
                assert_eq!(balance(&mut conn, id(STALL)?).await?, 0);
                // E27: the 30 left went back.
                assert_eq!(balance(&mut conn, id(HANA)?).await?, wallet + 3_000);
            }
        }
        assert_eq!(
            balance(&mut conn, w.voucher.voucher.escrow_account_id).await?,
            0
        );
        assert_eq!(escrow_findings(&mut conn).await?, vec![]);
        drop(conn);
        w.db.drop().await?;
    }
    Ok(())
}

/// A receipt racing the return: exactly one outcome, and the money adds
/// up either way (settled from escrow, then the rest returned; or returned,
/// then paid late from the wallet).
#[tokio::test]
async fn an_expiry_racing_a_receipt_has_one_outcome() -> TestResult {
    for _ in 0..3 {
        let Some(w) = world(10_000, 0).await? else {
            return Ok(());
        };
        let w = Arc::new(w);
        let pool = w.db.app_pool_of(4).await?;
        let mut conn = pool.acquire().await?;
        w.tick(&mut conn, NOW + 7 * DAY).await?;
        let wallet = balance(&mut conn, id(HANA)?).await?;
        drop(conn);
        let receipt = Arc::new(w.chain(&[(STALL, 6_000)])?.remove(0));
        let (wa, pa, ra) = (w.clone(), pool.clone(), receipt.clone());
        let upload = tokio::spawn(async move {
            let mut conn = pa.acquire().await.map_err(|e| e.to_string())?;
            wa.upload_at(&mut conn, &ra, RETURN_AT)
                .await
                .map(|r| r.outcome)
                .map_err(|e| e.to_string())
        });
        let (wb, pb) = (w.clone(), pool.clone());
        let tick = tokio::spawn(async move {
            let mut conn = pb.acquire().await.map_err(|e| e.to_string())?;
            wb.tick(&mut conn, RETURN_AT)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        let outcome = upload.await??;
        tick.await??;
        let mut conn = pool.acquire().await?;
        assert!(
            matches!(outcome, Outcome::Settled | Outcome::SettledLate),
            "{outcome:?}"
        );
        assert_eq!(balance(&mut conn, id(STALL)?).await?, 6_000);
        assert_eq!(
            balance(&mut conn, w.voucher.voucher.escrow_account_id).await?,
            0
        );
        // 100 reserved, 60 paid: Hana is 40 better off than when grace began.
        assert_eq!(balance(&mut conn, id(HANA)?).await?, wallet + 4_000);
        assert_eq!(escrow_findings(&mut conn).await?, vec![]);
        drop((conn, pool));
        Arc::try_unwrap(w)
            .map_err(|_| "world still shared")?
            .db
            .drop()
            .await?;
    }
    Ok(())
}
