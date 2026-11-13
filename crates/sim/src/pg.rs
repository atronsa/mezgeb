//! The same operations on PostgreSQL, through the engine, with faults.
//!
//! Before each attempt the lane draws whether to inject a fault and at which
//! point ([`FaultPoint`]). A faulted attempt is retried, as a client would,
//! with a fresh entry ID and the same idempotency key, until one attempt
//! runs without a fault. Whatever the faults did, that last answer must be
//! exactly what the reference ledger says.

use mezgeb_core::{EntryId, IdempotencyKey, LedgerError, PositiveAmount};
use mezgeb_engine::{
    CaptureHold, FaultPoint, HoldRequest, PlaceHold, PostOptions, PostRequest, capture_hold,
    expire_hold, place_hold, post_entry_with, release_hold, reverse_entry_with,
};
use sqlx::PgConnection;

use crate::workload::{Error, Lane, Op, Outcome, Shape};

/// Faults injected, by point.
#[derive(Debug, Default, Clone, Copy)]
pub struct Faults {
    pub fired: [u64; 4],
}

impl Faults {
    pub fn total(&self) -> u64 {
        self.fired.iter().sum()
    }

    pub fn add(&mut self, other: &Faults) {
        for (a, b) in self.fired.iter_mut().zip(other.fired) {
            *a += b;
        }
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

/// One attempt at `op` on PostgreSQL.
async fn attempt(
    conn: &mut PgConnection,
    lane: &Lane,
    op: &Op,
    entry: EntryId,
    key: &IdempotencyKey,
    options: &PostOptions,
) -> Result<Result<Outcome, LedgerError>, Error> {
    let positive = |amount: i64| {
        PositiveAmount::new(mezgeb_core::Money::from_minor(
            amount,
            mezgeb_core::Currency::ETB,
        ))
    };
    let hold_request = HoldRequest {
        entry_id: entry,
        client_id: lane.client,
        idempotency_key: key.clone(),
    };
    let posted =
        |r: Result<mezgeb_core::PostedEntry, LedgerError>| r.map(|e| Outcome::Entry(Shape::of(&e)));
    Ok(match *op {
        Op::Deposit { .. } | Op::Transfer { .. } | Op::Adjust { .. } => {
            let (draft, authorization) = lane.posting(op)?;
            let mut request = PostRequest::new(entry, lane.client, key.clone(), draft);
            if let Some(authorization) = authorization {
                request = request.authorized(authorization);
            }
            posted(post_entry_with(conn, request, options).await)
        }
        Op::Reverse { target } => {
            let request = PostRequest::new(entry, lane.client, key.clone(), Default::default())
                .authorized(Lane::booking_error()?);
            posted(reverse_entry_with(conn, target, request, options).await)
        }
        Op::Place {
            hold,
            wallet,
            held,
            amount,
            expires_at,
        } => {
            let place = PlaceHold {
                hold,
                account: lane.wallet(wallet)?,
                held,
                amount: positive(amount)?,
                expires_at,
            };
            place_hold(conn, &hold_request, place, options)
                .await
                .map(Outcome::Hold)
        }
        Op::Capture {
            hold,
            destination,
            amount,
            now,
        } => {
            let capture = CaptureHold {
                hold,
                destination,
                amount: positive(amount)?,
                now,
            };
            capture_hold(conn, &hold_request, capture, options)
                .await
                .map(Outcome::Hold)
        }
        Op::Release { hold } => release_hold(conn, &hold_request, hold, options)
            .await
            .map(Outcome::Hold),
        Op::Expire { hold, now } => expire_hold(conn, &hold_request, hold, now, options)
            .await
            .map(Outcome::Hold),
        Op::Tick => Ok(Outcome::Nothing),
    })
}

/// Runs `op` until an attempt without a fault answers; faults are drawn at
/// `fault_per_mille` per attempt.
pub async fn run(
    conn: &mut PgConnection,
    lane: &mut Lane,
    op: &Op,
    key: &IdempotencyKey,
    fault_per_mille: u64,
    faults: &mut Faults,
) -> Result<Outcome, Error> {
    loop {
        let fault = (lane.rng().below(1_000) < fault_per_mille)
            .then(|| FaultPoint::ALL[lane.rng().below(4) as usize]);
        let options = PostOptions {
            fault,
            ..PostOptions::default()
        };
        let entry = lane.fresh_entry()?;
        match attempt(conn, lane, op, entry, key, &options).await? {
            Err(LedgerError::Storage(message)) if message.starts_with("test hook") => {
                if let Some(point) = fault {
                    faults.fired[index(point)] += 1;
                }
            }
            Err(LedgerError::Storage(message)) => {
                return Err(format!("lane {}: storage error: {message}", lane.number).into());
            }
            Err(refused) => return Ok(Outcome::Refused(refused)),
            Ok(outcome) => return Ok(outcome),
        }
    }
}
