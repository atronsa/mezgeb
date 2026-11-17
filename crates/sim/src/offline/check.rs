//! The offline invariants, checked against the model after every step.
//!
//! The checker knows nothing of how the model decides: it looks only at
//! what was paid, from where, and what the vouchers and escrows hold, and
//! at the simulation's ground truth (which receipts a real device key
//! signed). That is how it catches a planted bug in the model, and how it
//! would catch the same bug in the engine through the comparison with
//! PostgreSQL.
//!
//! | Invariant | Checked here |
//! |---|---|
//! | INV-19 | No voucher pays more from escrow than its amount; an open voucher's escrow holds its amount less what it paid |
//! | INV-20 | No recorded receipt is paid twice, and no receipt's digest twice |
//! | INV-21 | Escrow never pays one counter twice; a conflicting copy freezes the voucher or is disputed |
//! | INV-22 | Every paid receipt was signed by its device's real key |
//! | INV-23 | A returned or ended voucher's escrow is zero |
//! | Chain | Every receipt settled by its link names its predecessor's digest |
//! | States | Vouchers move only forward; an ended voucher stays ended |
//! | Losses | The loss account holds exactly the unpaid shortfalls |

use std::collections::{BTreeMap, BTreeSet};

use mezgeb_core::{DeviceId, VoucherId};
use mezgeb_engine::VoucherStatus;
use mezgeb_offline::Outcome;

use super::model::Model;

/// What the simulation knows that the ledger cannot: the digests of every
/// body a real device key signed.
#[derive(Debug, Default)]
pub struct Truth {
    pub signed: BTreeSet<[u8; 32]>,
}

/// The checker's memory between steps.
#[derive(Debug, Default)]
pub struct Checker {
    /// Payments already audited.
    seen: usize,
    /// Transitions already audited.
    transitions: usize,
    recorded: BTreeSet<(DeviceId, u64)>,
    from_escrow: BTreeSet<(DeviceId, u64)>,
    digests: BTreeSet<[u8; 32]>,
    escrow_paid: BTreeMap<VoucherId, i64>,
}

fn allowed(from: VoucherStatus, to: VoucherStatus) -> bool {
    use VoucherStatus::{Active, Closed, Ended, Frozen, Grace, Returned};
    matches!(
        (from, to),
        (Active, Grace)
            | (Active | Grace, Frozen)
            | (Grace | Frozen, Returned)
            | (Returned | Closed, Ended)
    )
}

impl Checker {
    /// Checks what changed since the last call: new payments, new status
    /// changes and the vouchers they touched.
    pub fn step(&mut self, model: &mut Model, truth: &Truth) -> Result<(), String> {
        for p in &model.payments[self.seen..] {
            let at = format!("{}:{}", p.device, p.counter);
            if !truth.signed.contains(&p.digest) {
                return Err(format!(
                    "INV-22: receipt {at} was paid but no device key signed it"
                ));
            }
            if !self.digests.insert(p.digest) {
                return Err(format!("INV-20: receipt {at} was paid twice"));
            }
            if !p.copy && !self.recorded.insert((p.device, p.counter)) {
                return Err(format!(
                    "INV-20: two different receipts recorded for {at} were both paid"
                ));
            }
            if p.parts.escrow > 0 {
                if !self.from_escrow.insert((p.device, p.counter)) {
                    return Err(format!("INV-21: escrow paid counter {at} twice"));
                }
                *self.escrow_paid.entry(p.voucher).or_default() += p.parts.escrow;
            }
        }
        self.seen = model.payments.len();
        for (v, from, to) in &model.transitions[self.transitions..] {
            if !allowed(*from, *to) {
                return Err(format!(
                    "voucher {v} went from {} to {}",
                    from.as_str(),
                    to.as_str()
                ));
            }
        }
        self.transitions = model.transitions.len();
        let dirty = std::mem::take(&mut model.dirty);
        for v in dirty {
            self.voucher(model, v)?;
        }
        let owed: i64 = model
            .shortfalls
            .iter()
            .filter(|s| !s.repaid)
            .map(|s| s.amount)
            .sum();
        let loss = model.balance(model.setup.loss_account);
        if loss != owed {
            return Err(format!(
                "the loss account holds {loss}, but {owed} of shortfalls are unpaid"
            ));
        }
        Ok(())
    }

    /// Every voucher, not only the changed ones.
    pub fn all(&mut self, model: &mut Model) -> Result<(), String> {
        let all: Vec<VoucherId> = model.vouchers.keys().copied().collect();
        for v in all {
            self.voucher(model, v)?;
        }
        Ok(())
    }

    fn voucher(&self, model: &Model, id: VoucherId) -> Result<(), String> {
        let Some(v) = model.vouchers.get(&id) else {
            return Ok(());
        };
        let amount = v.voucher.amount.amount();
        let paid = self.escrow_paid.get(&id).copied().unwrap_or(0);
        if paid > amount {
            return Err(format!(
                "INV-19: voucher {id} paid {paid} from an escrow of {amount}"
            ));
        }
        let escrow = model.balance(v.voucher.escrow_account_id);
        let expected = if matches!(
            v.status,
            VoucherStatus::Active | VoucherStatus::Grace | VoucherStatus::Frozen
        ) {
            amount - paid
        } else {
            0
        };
        if escrow != expected {
            let rule = if expected == 0 { "INV-23" } else { "INV-19" };
            return Err(format!(
                "{rule}: voucher {id} ({}) has {escrow} in escrow, {expected} expected",
                v.status.as_str()
            ));
        }
        // Each receipt settled by its link names its predecessor.
        let device = v.voucher.device_id;
        let start = v.voucher.counter_start;
        for ((_, counter), r) in model.receipts.range((device, start)..=(device, u64::MAX)) {
            if r.voucher != id
                || r.gap_skipped
                || r.by_dispute
                || !matches!(r.outcome, Outcome::Settled | Outcome::SettledLate)
            {
                continue;
            }
            let link = if *counter == start {
                Some(v.voucher.digest())
            } else {
                model.receipts.get(&(device, counter - 1)).map(|p| p.digest)
            };
            if link != Some(r.prev_digest) {
                return Err(format!(
                    "chain: receipt {device}:{counter} settled with a link to a receipt that is not its predecessor"
                ));
            }
        }
        Ok(())
    }
}

/// At the end of an epoch, after every receipt was uploaded, every dispute
/// resolved and every voucher ended: nothing is left undecided.
pub fn settled_or_refused(model: &Model) -> Result<(), String> {
    for ((device, counter), r) in &model.receipts {
        if !matches!(
            r.outcome,
            Outcome::Settled | Outcome::SettledLate | Outcome::Refused | Outcome::Expired
        ) {
            let voucher = model.vouchers.get(&r.voucher);
            let disputes: Vec<String> = model
                .disputes
                .iter()
                .filter(|d| d.voucher == r.voucher || d.device == *device)
                .map(|d| {
                    format!(
                        "{} on voucher {} at {}: {:?}",
                        d.reason.as_str(),
                        d.voucher,
                        d.counter,
                        d.resolution
                    )
                })
                .collect();
            return Err(format!(
                "receipt {device}:{counter} is still {} after everything was uploaded and                  resolved; its voucher {} is {} (counters from {}), the device is {}; disputes: {}",
                r.outcome.as_str(),
                r.voucher,
                voucher.map_or("missing", |v| v.status.as_str()),
                voucher.map_or(0, |v| v.voucher.counter_start),
                model
                    .devices
                    .get(device)
                    .map_or("missing", |d| d.status.as_str()),
                if disputes.is_empty() {
                    "none".to_owned()
                } else {
                    disputes.join("; ")
                }
            ));
        }
    }
    for v in model.vouchers.values() {
        if v.status != VoucherStatus::Ended {
            return Err(format!(
                "voucher {} is still {} at the end",
                v.voucher.voucher_id,
                v.status.as_str()
            ));
        }
    }
    Ok(())
}
