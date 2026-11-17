//! The offline workload (simulator v2, step 2.14): fleets of devices paying
//! offline, against the reference model, and against PostgreSQL.
//!
//! **Lanes and epochs.** As in v1, the run is split into independent lanes,
//! each its own client, run in parallel. Each lane's run is split again
//! into epochs of `--epoch` operations: an epoch is a fresh client with its
//! own fleet, which keeps the reference ledger small however long the run
//! is. An epoch ends with a drain: every receipt still held is uploaded,
//! every dispute resolved, and time runs past every deadline. Then nothing
//! may be left undecided.
//!
//! **Checks.** After every operation the checker ([`check`]) audits what
//! changed: INV-19 to INV-23, the chains, voucher states and the loss
//! account. Every 1,000 operations and at the end of each epoch the
//! reference ledger's own checker runs (INV-1 to INV-8 and INV-13 to
//! INV-25 in memory). Against PostgreSQL, every answer is compared too,
//! and a full verification (INV-1 to INV-12, the hash chains) ends the run.
//!
//! **Deterministic.** The same options give the same operations and the
//! same answers on any machine. The run's digest, a SHA-256 of every
//! operation and answer, shows it: compare it across machines.

pub mod books;
pub mod check;
pub mod fleet;
pub mod ids;
pub mod model;
pub mod pg;

use std::collections::BTreeMap;

use mezgeb_core::check_invariants;
use sha2::{Digest, Sha256};

use self::check::Checker;
use self::fleet::{Answer, Op, Shape, World, apply};
use self::model::{Bug, Model};

/// How often the reference ledger's full checker runs, in operations.
const CHECK_EVERY: u64 = 1_000;
/// Rounds of resolving and ticking at the end of an epoch.
const DRAIN_ROUNDS: u32 = 20;

#[derive(Debug, Clone)]
pub struct Config {
    pub seed: u64,
    pub ops: u64,
    pub lanes: u64,
    pub devices: u64,
    pub epoch: u64,
    pub bug: Option<Bug>,
}

impl Config {
    pub fn shape(&self) -> Shape {
        Shape {
            seed: self.seed,
            devices: self.devices,
        }
    }

    /// The random operations of lane `n`: an even share.
    pub fn share(&self, n: u64) -> u64 {
        self.ops / self.lanes + u64::from(n < self.ops % self.lanes)
    }
}

/// What a run did.
#[derive(Debug, Default, Clone)]
pub struct Report {
    pub ops: BTreeMap<&'static str, u64>,
    pub outcomes: BTreeMap<&'static str, u64>,
    pub epochs: u64,
    pub steps: u64,
    pub full_checks: u64,
    pub devices: u64,
    pub digest: [u8; 32],
}

impl Report {
    pub fn add(&mut self, other: &Report) {
        for (k, n) in &other.ops {
            *self.ops.entry(k).or_default() += n;
        }
        for (k, n) in &other.outcomes {
            *self.outcomes.entry(k).or_default() += n;
        }
        self.epochs += other.epochs;
        self.steps += other.steps;
        self.full_checks += other.full_checks;
        self.devices += other.devices;
    }

    pub fn digest_hex(&self) -> String {
        mezgeb_core::chain::hex(&self.digest)
    }
}

/// Where a run failed.
#[derive(Debug, Clone)]
pub struct Failure {
    pub lane: u64,
    pub epoch: u64,
    pub step: u64,
    pub message: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "lane {} epoch {} step {}: {}",
            self.lane, self.epoch, self.step, self.message
        )
    }
}

/// One epoch of one lane: its world, the model, and what they did.
pub struct Epoch {
    pub world: World,
    pub model: Model,
    checker: Checker,
    hasher: Sha256,
    pub report: Report,
    pub step: u64,
    drain_rounds: u32,
}

impl Epoch {
    pub fn new(config: &Config, lane: u64, epoch: u64) -> Result<Epoch, String> {
        let world = World::new(config.shape(), lane, epoch)?;
        let mut model = Model::new(world.setup.clone());
        model.bug = config.bug;
        Ok(Epoch {
            report: Report {
                epochs: 1,
                devices: config.devices,
                ..Report::default()
            },
            world,
            model,
            checker: Checker::default(),
            hasher: Sha256::new(),
            step: 0,
            drain_rounds: 0,
        })
    }

    pub fn failure(&self, message: impl Into<String>) -> Failure {
        Failure {
            lane: self.world.ids.lane,
            epoch: self.world.ids.epoch,
            step: self.step,
            message: message.into(),
        }
    }

    /// Applies one operation to the model and checks the result.
    pub fn apply(&mut self, kind: &'static str, op: &Op) -> Result<Answer, String> {
        let answer = apply(&mut self.model, op);
        self.world.after(op, &answer);
        self.checker
            .step(&mut self.model, &self.world.truth)
            .map_err(|e| format!("after {kind}: {e}"))?;
        self.hasher
            .update(format!("{} {kind} {answer:?}\n", self.step).as_bytes());
        *self.report.ops.entry(kind).or_default() += 1;
        match &answer {
            Answer::Uploaded(i) => {
                *self.report.outcomes.entry(i.outcome.as_str()).or_default() += 1
            }
            Answer::Issued(Ok(_)) => *self.report.outcomes.entry("ISSUED").or_default() += 1,
            Answer::Issued(Err(e)) => {
                let why = match e.split(['(', ' ']).next().unwrap_or_default() {
                    "DeviceNotActive" => "REFUSED_DEVICE_NOT_ACTIVE",
                    "VoucherOpen" => "REFUSED_VOUCHER_OPEN",
                    "OverTierLimit" => "REFUSED_OVER_TIER_LIMIT",
                    "OverCustomerCap" => "REFUSED_OVER_CUSTOMER_CAP",
                    "ShortfallUnpaid" => "REFUSED_SHORTFALL_UNPAID",
                    "TierDisabled" => "REFUSED_TIER_DISABLED",
                    "Ledger" => "REFUSED_INSUFFICIENT_FUNDS",
                    "DeviceNotFound" => "REFUSED_DEVICE_NOT_REGISTERED",
                    _ => "REFUSED_OTHER",
                };
                *self.report.outcomes.entry(why).or_default() += 1;
            }
            _ => {}
        }
        self.step += 1;
        self.report.steps += 1;
        if self.step.is_multiple_of(CHECK_EVERY) {
            self.full_check()?;
        }
        Ok(answer)
    }

    /// An operation that stays on devices: counted, nothing to check.
    pub fn offline(&mut self, kind: &'static str) {
        self.hasher
            .update(format!("{} {kind}\n", self.step).as_bytes());
        *self.report.ops.entry(kind).or_default() += 1;
        self.step += 1;
        self.report.steps += 1;
    }

    fn full_check(&mut self) -> Result<(), String> {
        check_invariants(&self.model.ledger).map_err(|v| format!("{v:?}"))?;
        self.checker.all(&mut self.model)?;
        self.report.full_checks += 1;
        Ok(())
    }

    /// The next operation of the drain at the end of the epoch, or `None`
    /// when it is done.
    pub fn next_drain(&mut self) -> Option<(&'static str, Op)> {
        if let Some(op) = self.world.drain_one() {
            return Some(("drain-upload", op));
        }
        if let Some(&dispute) = self.model.open_disputes().first() {
            return Some((
                "drain-resolve",
                Op::Resolve {
                    dispute,
                    resolution: if dispute % 3 == 0 {
                        mezgeb_engine::DisputeResolution::Refused
                    } else {
                        mezgeb_engine::DisputeResolution::Paid
                    },
                    now: self.world.now,
                },
            ));
        }
        let open = self
            .model
            .vouchers
            .values()
            .any(|v| v.status != mezgeb_engine::VoucherStatus::Ended);
        if open && self.drain_rounds < DRAIN_ROUNDS {
            self.drain_rounds += 1;
            return Some(("drain-tick", self.world.far_tick()));
        }
        None
    }

    /// The checks at the end of an epoch.
    pub fn finish(&mut self) -> Result<(), String> {
        self.full_check()?;
        check::settled_or_refused(&self.model)?;
        self.report.digest = self.hasher.clone().finalize().into();
        Ok(())
    }
}

/// One lane against the model alone.
fn run_lane(config: &Config, lane: u64) -> Result<Report, Failure> {
    let share = config.share(lane);
    let mut report = Report::default();
    let mut lane_hash = Sha256::new();
    let mut done = 0;
    let mut epoch = 0;
    while done < share {
        let ops = config.epoch.min(share - done);
        let mut e = Epoch::new(config, lane, epoch).map_err(|m| Failure {
            lane,
            epoch,
            step: 0,
            message: m,
        })?;
        for op in e.world.setup_ops().map_err(|m| e.failure(m))? {
            e.apply(op.kind(), &op).map_err(|m| e.failure(m))?;
        }
        for _ in 0..ops {
            let (kind, op) = e.world.next(&e.model).map_err(|m| e.failure(m))?;
            match op {
                Some(op) => {
                    e.apply(kind, &op).map_err(|m| e.failure(m))?;
                }
                None => e.offline(kind),
            }
        }
        while let Some((kind, op)) = e.next_drain() {
            e.apply(kind, &op).map_err(|m| e.failure(m))?;
        }
        e.finish().map_err(|m| e.failure(m))?;
        lane_hash.update(e.report.digest);
        report.add(&e.report);
        done += ops;
        epoch += 1;
    }
    report.digest = lane_hash.finalize().into();
    Ok(report)
}

/// Every lane against the model, in parallel threads. The first failure in
/// lane order is reported, so the report does not depend on which thread
/// ran first.
pub fn run_reference(config: &Config) -> Result<Report, Failure> {
    let results: Vec<Result<Report, Failure>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..config.lanes)
            .map(|lane| scope.spawn(move || run_lane(config, lane)))
            .collect();
        handles
            .into_iter()
            .enumerate()
            .map(|(lane, h)| {
                h.join().unwrap_or_else(|_| {
                    Err(Failure {
                        lane: lane as u64,
                        epoch: 0,
                        step: 0,
                        message: "the lane panicked".into(),
                    })
                })
            })
            .collect()
    });
    let mut total = Report::default();
    let mut hash = Sha256::new();
    for result in results {
        let report = result?;
        hash.update(report.digest);
        total.add(&report);
    }
    total.digest = hash.finalize().into();
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(ops: u64, bug: Option<Bug>) -> Config {
        Config {
            seed: 1,
            ops,
            lanes: 2,
            devices: 16,
            epoch: 3_000,
            bug,
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_run() -> Result<(), String> {
        let first = run_reference(&config(6_000, None)).map_err(|f| f.to_string())?;
        let second = run_reference(&config(6_000, None)).map_err(|f| f.to_string())?;
        assert_eq!(first.digest, second.digest);
        assert_eq!(first.ops, second.ops);
        for outcome in ["SETTLED", "DUPLICATE", "WAITING_FOR_GAP", "BAD_SIGNATURE"] {
            assert!(
                first.outcomes.get(outcome).copied().unwrap_or(0) > 0,
                "no {outcome} in {:?}",
                first.outcomes
            );
        }
        Ok(())
    }

    /// The exit gate of step 2.14: a run reproduces identically on another
    /// machine. This digest was computed on Windows (x86-64); CI checks it
    /// on Linux on every change. If an intended change to the workload, the
    /// model or the engine's rules changes it, update it here and say why.
    #[test]
    fn the_digest_is_the_same_on_every_machine() -> Result<(), String> {
        let report = run_reference(&config(6_000, None)).map_err(|f| f.to_string())?;
        assert_eq!(
            report.digest_hex(),
            "56d4a2253285e6b004d51e748418b4bc5e69914a7d4b227dfd0ae57a9ce216b4"
        );
        Ok(())
    }

    #[test]
    fn planted_bugs_are_caught() {
        for bug in [Bug::SkipCounterCheck, Bug::SkipChainCheck] {
            let result = run_reference(&Config {
                ops: 40_000,
                lanes: 4,
                ..config(0, Some(bug))
            });
            assert!(result.is_err(), "{bug:?} was not caught");
        }
    }
}
