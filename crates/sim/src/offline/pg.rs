//! The offline workload against PostgreSQL, through the engine, with
//! crashes.
//!
//! Every operation runs on the model first, then through the engine, and
//! the two answers must be equal. Before each attempt a fault is drawn
//! (`--faults` per thousand) at one of the engine's four fault points: a
//! crash before the commit rolls the attempt back; a crash after it loses
//! the response. The attempt is retried, as a client would, until one runs
//! without a fault:
//!
//! - an issue, a posting, a repayment or a dispute's end is idempotent, so
//!   the retry answers as the model did;
//! - an upload whose first attempt committed answers `DUPLICATE` on retry:
//!   accepted when the model recorded the receipt;
//! - a tick that crashed part-way is finished by the retry; its report is
//!   not compared, its effect is, by the state checks.
//!
//! The faults are drawn from their own random stream, so the operations are
//! the same as in a run against the model alone.
//!
//! **State checks.** Every 5,000 operations every account and device of
//! the lane is compared, and at the end of each epoch also every voucher,
//! receipt, shortfall and the entry count. At the end of the run every
//! escrow must hold its voucher's unspent value, and a full verification
//! (INV-1 to INV-12) and the hash chains must be clean.
//!
//! **A database per lane.** The offline tick moves every client's
//! vouchers, so lanes, each with its own clock, cannot share a database.

use std::time::Duration;

use mezgeb_core::{Chart, Currency, EntryDraft, IdempotencyKey, Money, PositiveAmount, Timestamp};
use mezgeb_engine::{
    Attestation, AttestationKind, DeviceStatus, FaultPoint, IssuerKey, MIGRATOR, NewDevice,
    NewVoucher, OfflineAccounts, PostOptions, PostRequest, Scope, Upload, VerifyOptions,
    create_client, disputes, escrow_findings, get_account, get_device, get_voucher, ingest_receipt,
    issue_offline_voucher, offline_tick, open_account_keyed, post_entry_with, register_device,
    repay_shortfall, resolve_dispute, set_device_status, set_late_deadline, set_offline_fee,
    set_tier_limits, shortfalls, verify, verify_chains, voucher_receipts,
};
use mezgeb_offline::{Outcome, Tier};
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, Executor, PgConnection, PgPool};

use super::books::{SimBooks, issue_book};
use super::fleet::{Answer, CHART, Op, START};
use super::model::{Ingested, Ticked, error_kind};
use super::{Config, Epoch, Failure, Report};
use crate::rng::SplitMix64;

/// How often a lane's accounts and devices are compared, in operations.
const COMPARE_EVERY: u64 = 5_000;

/// Faults injected, by point, and those that lost a response.
#[derive(Debug, Default, Clone, Copy)]
pub struct Faults {
    pub fired: [u64; 4],
}

impl Faults {
    pub fn total(&self) -> u64 {
        self.fired.iter().sum()
    }
}

fn index(point: FaultPoint) -> usize {
    match point {
        FaultPoint::BeforeLock => 0,
        FaultPoint::AfterLines => 1,
        FaultPoint::BeforeCommit => 2,
        FaultPoint::AfterCommit => 3,
    }
}

fn is_hook(error: &str) -> bool {
    error.contains("test hook")
}

/// One lane's connection to PostgreSQL in one epoch.
struct Lane<'a> {
    conn: PgConnection,
    rng: SplitMix64,
    faults: &'a mut Faults,
    per_mille: u64,
    chart: Chart,
    issuer: IssuerKey,
    accounts: OfflineAccounts,
    /// Retries so far: each retry of a posting or an issue gets a fresh
    /// entry ID, as the API mints one per request, and the same
    /// idempotency key.
    retries: u64,
}

impl Lane<'_> {
    fn options(&mut self) -> (PostOptions, Option<FaultPoint>) {
        let fault = (self.rng.below(1_000) < self.per_mille)
            .then(|| FaultPoint::ALL[self.rng.below(4) as usize]);
        (
            PostOptions {
                fault,
                ..PostOptions::default()
            },
            fault,
        )
    }

    fn fired(&mut self, fault: Option<FaultPoint>) {
        if let Some(point) = fault {
            self.faults.fired[index(point)] += 1;
        }
    }
}

/// The engine's answer to `op`, retried past injected faults. Also says
/// whether a fault lost a response after its commit, and whether any
/// fault fired.
#[allow(clippy::too_many_lines)]
async fn run(lane: &mut Lane<'_>, e: &Epoch, op: &Op) -> Result<(Answer, bool, bool), String> {
    let client = e.world.client;
    let mut lost_response = false;
    let mut any = false;
    let mut first = true;
    loop {
        let (options, fault) = lane.options();
        let entry_for = |entry: mezgeb_core::EntryId, lane: &mut Lane<'_>| {
            if first {
                return Ok(entry);
            }
            lane.retries += 1;
            e.world
                .ids
                .get::<mezgeb_core::EntryId>(super::ids::Kind::Entry, (1 << 20) | lane.retries)
        };
        let fresh = match op {
            Op::Post { entry, .. } => Some(entry_for(*entry, lane)?),
            Op::Issue(r) => Some(entry_for(r.entry, lane)?),
            _ => None,
        };
        first = false;
        let conn = &mut lane.conn;
        let result: Result<Answer, String> = match op {
            Op::Open { id, reference } => {
                let key =
                    IdempotencyKey::new(format!("open-{reference}")).map_err(|e| e.to_string())?;
                Ok(Answer::Done(
                    open_account_keyed(conn, client, &key, *id, &lane.chart, reference)
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("{e:?}")),
                ))
            }
            Op::Register {
                device,
                owner,
                tier,
                key,
            } => {
                let kind = match tier {
                    Tier::A => AttestationKind::Card,
                    Tier::B => AttestationKind::Android,
                    Tier::C => AttestationKind::None,
                };
                let new = NewDevice {
                    id: *device,
                    client,
                    owner_account: *owner,
                    public_key: key.public_key().clone(),
                    attestation: Attestation {
                        kind,
                        tier: *tier,
                        detail: "simulated".into(),
                    },
                    label: "simulated device".into(),
                    now: Timestamp::from_micros(START),
                };
                Ok(Answer::Done(
                    register_device(conn, &lane.issuer, new)
                        .await
                        .map(|_| ())
                        .map_err(|e| error_kind(&e)),
                ))
            }
            Op::Post {
                entry,
                key,
                debit,
                credit,
                amount,
            } => {
                let money = Money::from_minor(*amount, Currency::ETB);
                let draft = EntryDraft::default()
                    .debit(*debit, money)
                    .credit(*credit, money);
                let key = IdempotencyKey::new(key.clone()).map_err(|e| e.to_string())?;
                let entry = fresh.unwrap_or(*entry);
                match post_entry_with(conn, PostRequest::new(entry, client, key, draft), &options)
                    .await
                {
                    Ok(_) => Ok(Answer::Done(Ok(()))),
                    Err(e) => {
                        let text = format!("{e:?}");
                        if is_hook(&text) {
                            Err(text)
                        } else {
                            Ok(Answer::Done(Err(text)))
                        }
                    }
                }
            }
            Op::Issue(r) => {
                let amount = PositiveAmount::new(Money::from_minor(r.amount, Currency::ETB))
                    .map_err(|e| e.to_string())?;
                let new = NewVoucher {
                    voucher_id: r.voucher,
                    device_id: r.device,
                    client,
                    entry_id: fresh.unwrap_or(r.entry),
                    escrow_account: r.escrow,
                    idempotency_key: IdempotencyKey::new(format!("voucher-{}", r.voucher))
                        .map_err(|e| e.to_string())?,
                    amount,
                    last_counter: r.last_counter,
                    now: Timestamp::from_micros(r.now),
                };
                match issue_offline_voucher(
                    conn,
                    &lane.issuer,
                    &lane.chart,
                    &new,
                    issue_book(r.voucher, amount),
                    &options,
                )
                .await
                {
                    Ok(issued) => Ok(Answer::Issued(Ok(issued.voucher))),
                    Err(e) if is_hook(&e.to_string()) => Err(e.to_string()),
                    Err(e) => Ok(Answer::Issued(Err(error_kind(&e)))),
                }
            }
            Op::Upload {
                body,
                signature,
                now,
            } => {
                let upload = Upload {
                    client,
                    body,
                    signature,
                    now: Timestamp::from_micros(*now),
                };
                match ingest_receipt(conn, &upload, lane.accounts, &SimBooks, &options).await {
                    Ok(r) => Ok(Answer::Uploaded(Ingested {
                        outcome: r.outcome,
                        settled: r.settled.iter().map(|s| s.counter).collect(),
                    })),
                    Err(e) if is_hook(&e.to_string()) => Err(e.to_string()),
                    Err(e) => return Err(format!("the upload failed: {e}")),
                }
            }
            Op::Tick { now } => {
                match offline_tick(
                    conn,
                    Timestamp::from_micros(*now),
                    lane.accounts,
                    &SimBooks,
                    &options,
                )
                .await
                {
                    Ok(r) => Ok(Answer::Ticked(Ticked {
                        settled_past_gaps: r
                            .settled_past_gaps
                            .iter()
                            .map(|(v, s)| (*v, s.iter().map(|x| x.counter).collect()))
                            .collect(),
                        graced: r.graced,
                        returned: r.returned.iter().map(|(v, e)| (*v, e.is_some())).collect(),
                        ended: r.ended,
                    })),
                    Err(e) if is_hook(&e.to_string()) => Err(e.to_string()),
                    Err(e) => return Err(format!("the tick failed: {e}")),
                }
            }
            Op::Resolve {
                dispute,
                resolution,
                now,
            } => {
                let voucher = e
                    .model
                    .disputes
                    .get(*dispute)
                    .ok_or("no such dispute in the model")?
                    .voucher;
                let found = disputes(conn, Some(client), true)
                    .await
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|d| d.voucher == voucher);
                match found {
                    None => Ok(Answer::Done(Err("DisputeNotFound".into()))),
                    Some(d) => match resolve_dispute(
                        conn,
                        d.id,
                        *resolution,
                        "sim:operator",
                        Timestamp::from_micros(*now),
                        lane.accounts,
                        &SimBooks,
                        &options,
                    )
                    .await
                    {
                        Ok(_) => Ok(Answer::Done(Ok(()))),
                        Err(e) if is_hook(&e.to_string()) => Err(e.to_string()),
                        Err(e) => Ok(Answer::Done(Err(error_kind(&e)))),
                    },
                }
            }
            Op::Repay { shortfall } => {
                let s = e
                    .model
                    .shortfalls
                    .get(*shortfall)
                    .ok_or("no such shortfall in the model")?;
                let same = |d, c, a| d == s.device && c == s.counter && a == s.amount;
                let nth = e.model.shortfalls[..*shortfall]
                    .iter()
                    .filter(|x| same(x.device, x.counter, x.amount))
                    .count();
                let found = shortfalls(conn, client, None)
                    .await
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .filter(|x| same(x.device, x.counter, x.amount.amount()))
                    .nth(nth);
                match found {
                    None => Ok(Answer::Done(Err("ShortfallNotFound".into()))),
                    Some(x) => {
                        match repay_shortfall(
                            conn,
                            client,
                            x.id,
                            lane.accounts,
                            &SimBooks,
                            &options,
                        )
                        .await
                        {
                            Ok(_) => Ok(Answer::Done(Ok(()))),
                            Err(e) if is_hook(&e.to_string()) => Err(e.to_string()),
                            Err(e) => Ok(Answer::Done(Err(error_kind(&e)))),
                        }
                    }
                }
            }
            Op::Lost { device } => Ok(Answer::Done(
                set_device_status(conn, Some(client), *device, DeviceStatus::Lost, Some("sim"))
                    .await
                    .map(|_| ())
                    .map_err(|e| error_kind(&e)),
            )),
        };
        match result {
            Ok(answer) => return Ok((answer, lost_response, any)),
            Err(hook) => {
                if !is_hook(&hook) {
                    return Err(hook);
                }
                any = true;
                lost_response |= fault == Some(FaultPoint::AfterCommit);
                lane.fired(fault);
            }
        }
    }
}

/// Whether the engine's answer is the model's, allowing for what a lost
/// response changes.
fn same(model: &Answer, pg: &Answer, lost_response: bool, any_fault: bool) -> bool {
    if model == pg {
        return true;
    }
    match (model, pg) {
        // The first attempt recorded it; the retry finds it recorded.
        (Answer::Uploaded(m), Answer::Uploaded(p)) if lost_response => {
            p.outcome == Outcome::Duplicate
                && !matches!(
                    m.outcome,
                    Outcome::BadFormat
                        | Outcome::BadSignature
                        | Outcome::UnknownDevice
                        | Outcome::UnknownVoucher
                        | Outcome::Duplicate
                )
        }
        // A tick finished by its retry reports only what the retry did.
        (Answer::Ticked(_), Answer::Ticked(_)) => any_fault,
        _ => false,
    }
}

/// A connection option set for one of the ledger's roles.
fn role(admin: &PgConnectOptions, database: &str, name: &str) -> PgConnectOptions {
    admin
        .clone()
        .database(database)
        .username(name)
        .password(name)
}

/// Compares the lane's accounts and devices; with `all`, also its
/// vouchers, receipts, shortfalls and entries.
async fn compare(conn: &mut PgConnection, e: &Epoch, all: bool) -> Result<(), String> {
    let client = e.world.client;
    for account in e.model.ledger.accounts() {
        let stored = get_account(conn, account.id())
            .await
            .map_err(|e| e.to_string())?;
        if stored.as_ref() != Some(account) {
            return Err(format!(
                "account {} differs: PostgreSQL {stored:?}, the model {account:?}",
                account.id()
            ));
        }
    }
    for (id, device) in &e.model.devices {
        let stored = get_device(conn, client, *id)
            .await
            .map_err(|e| e.to_string())?;
        if (stored.status, stored.next_counter) != (device.status, device.next_counter) {
            return Err(format!(
                "device {id} differs: PostgreSQL {} from {}, the model {} from {}",
                stored.status.as_str(),
                stored.next_counter,
                device.status.as_str(),
                device.next_counter
            ));
        }
    }
    if !all {
        return Ok(());
    }
    for (id, voucher) in &e.model.vouchers {
        let stored = get_voucher(conn, client, *id)
            .await
            .map_err(|e| e.to_string())?;
        if stored.status != voucher.status || stored.voucher != voucher.voucher {
            return Err(format!(
                "voucher {id} differs: PostgreSQL {}, the model {}",
                stored.status.as_str(),
                voucher.status.as_str()
            ));
        }
        let pg: Vec<(u64, Outcome)> = voucher_receipts(conn, client, *id)
            .await
            .map_err(|e| e.to_string())?
            .iter()
            .map(|r| (r.counter, r.outcome))
            .collect();
        let device = voucher.voucher.device_id;
        let model: Vec<(u64, Outcome)> = e
            .model
            .receipts
            .range((device, 0)..=(device, u64::MAX))
            .filter(|(_, r)| r.voucher == *id)
            .map(|((_, c), r)| (*c, r.outcome))
            .collect();
        if pg != model {
            return Err(format!(
                "voucher {id}'s receipts differ: PostgreSQL {pg:?}, the model {model:?}"
            ));
        }
    }
    let mut pg: Vec<(String, u64, i64, bool)> = shortfalls(conn, client, None)
        .await
        .map_err(|e| e.to_string())?
        .iter()
        .map(|s| {
            (
                s.device.to_string(),
                s.counter,
                s.amount.amount(),
                s.repaid_entry.is_some(),
            )
        })
        .collect();
    let mut model: Vec<(String, u64, i64, bool)> = e
        .model
        .shortfalls
        .iter()
        .map(|s| (s.device.to_string(), s.counter, s.amount, s.repaid))
        .collect();
    pg.sort();
    model.sort();
    if pg != model {
        return Err(format!(
            "the shortfalls differ: PostgreSQL {pg:?}, the model {model:?}"
        ));
    }
    let entries: i64 =
        sqlx::query_scalar("SELECT count(*) FROM journal_entries WHERE client_id = $1")
            .bind(client.as_uuid())
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| e.to_string())?;
    if usize::try_from(entries).unwrap_or(usize::MAX) != e.model.ledger.entries().len() {
        return Err(format!(
            "{entries} entries in PostgreSQL, {} in the model",
            e.model.ledger.entries().len()
        ));
    }
    Ok(())
}

/// One operation on the model, then on PostgreSQL.
async fn step(
    lane: &mut Lane<'_>,
    e: &mut Epoch,
    kind: &'static str,
    op: &Op,
) -> Result<(), String> {
    let expected = e.apply(kind, op)?;
    // A dispute or shortfall is found in PostgreSQL by what never changes
    // in the model: its voucher, or its device, counter and amount.
    let (actual, lost, any) = run(lane, e, op).await?;
    if !same(&expected, &actual, lost, any) {
        return Err(format!(
            "{kind}: PostgreSQL answered {actual:?}, the model {expected:?}"
        ));
    }
    if any && matches!(op, Op::Tick { .. }) {
        compare(&mut lane.conn, e, false).await?;
    }
    if e.step.is_multiple_of(COMPARE_EVERY) {
        compare(&mut lane.conn, e, false).await?;
    }
    Ok(())
}

/// One lane's epochs against PostgreSQL.
async fn run_lane(
    config: Config,
    lane: u64,
    per_mille: u64,
    pool: PgPool,
    admin: PgConnectOptions,
) -> Result<(Report, Faults), Failure> {
    let share = config.share(lane);
    let mut report = Report::default();
    let mut faults = Faults::default();
    let mut lane_hash = Sha256::new();
    let mut done = 0;
    let mut epoch = 0;
    let fail = |epoch, message: String| Failure {
        lane,
        epoch,
        step: 0,
        message,
    };
    while done < share {
        let ops = config.epoch.min(share - done);
        let mut e = Epoch::new(&config, lane, epoch).map_err(|m| fail(epoch, m))?;
        // The client and its settings, as the administrator.
        let setup = async {
            let mut admin = PgConnection::connect_with(&admin)
                .await
                .map_err(|e| e.to_string())?;
            let s = &e.world.setup;
            create_client(
                &mut admin,
                s.client,
                &format!("sim-l{lane}-e{epoch}"),
                100_000,
                100_000,
            )
            .await
            .map_err(|e| e.to_string())?;
            set_offline_fee(&mut admin, s.client, s.fee_bps)
                .await
                .map_err(|e| e.to_string())?;
            set_late_deadline(
                &mut admin,
                s.client,
                u32::try_from(s.late_days).unwrap_or(90),
            )
            .await
            .map_err(|e| e.to_string())?;
            for (tier, limits) in &s.tiers {
                set_tier_limits(&mut admin, s.client, *tier, *limits)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            admin.close().await.map_err(|e| e.to_string())?;
            Ok::<_, String>(())
        };
        setup.await.map_err(|m| fail(epoch, m))?;
        let conn = pool
            .acquire()
            .await
            .map_err(|e| fail(epoch, e.to_string()))?
            .detach();
        let mut l = Lane {
            conn,
            rng: SplitMix64::new(
                config.seed ^ 0xFA17 ^ lane.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (epoch << 32),
            ),
            faults: &mut faults,
            per_mille,
            chart: Chart::from_toml(CHART).map_err(|e| fail(epoch, e.to_string()))?,
            issuer: IssuerKey::from_secret(e.world.setup.issuer.clone()),
            accounts: OfflineAccounts {
                fee_account: Some(e.world.setup.fee_account),
                loss_account: Some(e.world.setup.loss_account),
            },
            retries: 0,
        };
        let result = async {
            for op in e.world.setup_ops()? {
                step(&mut l, &mut e, op.kind(), &op).await?;
            }
            for _ in 0..ops {
                match e.world.next(&e.model)? {
                    (kind, Some(op)) => step(&mut l, &mut e, kind, &op).await?,
                    (kind, None) => e.offline(kind),
                }
            }
            while let Some((kind, op)) = e.next_drain() {
                step(&mut l, &mut e, kind, &op).await?;
            }
            e.finish()?;
            compare(&mut l.conn, &e, true).await
        }
        .await;
        result.map_err(|m| e.failure(m))?;
        lane_hash.update(e.report.digest);
        report.add(&e.report);
        done += ops;
        epoch += 1;
    }
    report.digest = lane_hash.finalize().into();
    Ok((report, faults))
}

fn failure(lane: u64, message: impl Into<String>) -> Failure {
    Failure {
        lane,
        epoch: 0,
        step: 0,
        message: message.into(),
    }
}

/// A throwaway database for each lane, migrated: the offline tick moves
/// every client's vouchers, so lanes cannot share one.
async fn create(admin: &PgConnectOptions, database: &str) -> Result<(), String> {
    let mut superuser = PgConnection::connect_with(admin)
        .await
        .map_err(|e| e.to_string())?;
    superuser
        .execute(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {database} OWNER mezgeb_owner"
        )))
        .await
        .map_err(|e| e.to_string())?;
    superuser.close().await.map_err(|e| e.to_string())?;
    let mut owner = PgConnection::connect_with(&role(admin, database, "mezgeb_owner"))
        .await
        .map_err(|e| e.to_string())?;
    MIGRATOR.run(&mut owner).await.map_err(|e| e.to_string())?;
    owner.close().await.map_err(|e| e.to_string())
}

/// A lane's database at the end: every escrow holds its value, and a full
/// verification and the hash chains are clean.
async fn verify_lane(admin: &PgConnectOptions, database: &str) -> Result<(), String> {
    let mut app = PgConnection::connect_with(&role(admin, database, "mezgeb_app"))
        .await
        .map_err(|e| e.to_string())?;
    let findings = escrow_findings(&mut app).await.map_err(|e| e.to_string())?;
    if !findings.is_empty() {
        return Err(format!(
            "escrows that do not hold their value: {findings:?}"
        ));
    }
    app.close().await.map_err(|e| e.to_string())?;
    let mut verifier = PgConnection::connect_with(&role(admin, database, "mezgeb_verifier"))
        .await
        .map_err(|e| e.to_string())?;
    let options = VerifyOptions {
        scope: Scope::Full,
        settle: Duration::ZERO,
        record: true,
    };
    let verification = verify(&mut verifier, &options)
        .await
        .map_err(|e| e.to_string())?;
    if !verification.is_clean() {
        return Err(format!(
            "verification is not clean:
{verification}"
        ));
    }
    let chains = verify_chains(&mut verifier, &[])
        .await
        .map_err(|e| e.to_string())?;
    if !chains.is_clean() {
        return Err(format!("the hash chains are broken: {:?}", chains.findings));
    }
    verifier.close().await.map_err(|e| e.to_string())
}

/// Every lane, each in its own throwaway database, in parallel. The first
/// failure in lane order is reported; a failed lane's database is kept for
/// inspection.
pub async fn run_database(
    config: &Config,
    per_mille: u64,
    url: &str,
) -> Result<(Report, Faults), Failure> {
    let admin: PgConnectOptions = url
        .parse()
        .map_err(|e: sqlx::Error| failure(0, e.to_string()))?;
    let databases: Vec<String> = (0..config.lanes)
        .map(|lane| {
            format!(
                "mezgeb_sim_offline_{}_{}_{lane}",
                config.seed,
                std::process::id()
            )
        })
        .collect();
    let mut tasks = Vec::new();
    for (lane, database) in (0..config.lanes).zip(&databases) {
        create(&admin, database)
            .await
            .map_err(|m| failure(lane, m))?;
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(600))
            .connect_with(role(&admin, database, "mezgeb_app"))
            .await
            .map_err(|e| failure(lane, e.to_string()))?;
        tasks.push(tokio::spawn(run_lane(
            config.clone(),
            lane,
            per_mille,
            pool,
            role(&admin, database, "mezgeb_admin"),
        )));
    }
    let mut total = Report::default();
    let mut hash = Sha256::new();
    let mut faults = Faults::default();
    let mut first: Option<Failure> = None;
    for ((lane, task), database) in (0..config.lanes).zip(tasks).zip(&databases) {
        let result = match task.await {
            Ok(Ok(done)) => verify_lane(&admin, database)
                .await
                .map(|()| done)
                .map_err(|m| failure(lane, m)),
            Ok(Err(f)) => Err(f),
            Err(_) => Err(failure(lane, "the lane panicked")),
        };
        match result {
            Ok((report, f)) => {
                hash.update(report.digest);
                total.add(&report);
                for (a, b) in faults.fired.iter_mut().zip(f.fired) {
                    *a += b;
                }
                let mut superuser = PgConnection::connect_with(&admin)
                    .await
                    .map_err(|e| failure(lane, e.to_string()))?;
                superuser
                    .execute(sqlx::AssertSqlSafe(format!(
                        "DROP DATABASE {database} WITH (FORCE)"
                    )))
                    .await
                    .map_err(|e| failure(lane, e.to_string()))?;
                let _ = superuser.close().await;
            }
            Err(f) => {
                eprintln!("kept database {database} for inspection");
                first.get_or_insert(f);
            }
        }
    }
    total.digest = hash.finalize().into();
    match first {
        Some(f) => Err(f),
        None => {
            println!(
                "every lane: escrows hold their value; full verification and hash chains clean"
            );
            Ok((total, faults))
        }
    }
}
