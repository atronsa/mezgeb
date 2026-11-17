//! The reference model of the offline ledger: protocol §7–§14 in memory.
//!
//! It makes the same decisions as the engine (`offline.rs`,
//! `offline_receipts.rs`, `offline_lifecycle.rs`), check by check and in
//! the same order, and posts the same entries to a [`MemLedger`]. Against
//! PostgreSQL every answer of the engine must equal the model's. On its own,
//! it runs millions of operations fast, and the invariant checker
//! ([`super::check`]) watches it.
//!
//! The model follows the engine even where the engine is questionable: the
//! checker, which knows nothing of how decisions are made, is what finds
//! the problem. A planted bug ([`Bug`]) changes the model only, to show the
//! checker catches it.

use std::collections::{BTreeMap, BTreeSet};

use mezgeb_core::{
    Account, AccountId, Chart, ClientId, Currency, DeviceId, EntryDraft, EntryId, LedgerError,
    MemLedger, Money, Timestamp, VoucherId,
};
use mezgeb_engine::{
    DeviceStatus, DisputeResolution, GAP_TIMEOUT_MICROS, OfflineError, Parts, PayRequest,
    RepayRequest, ReturnRequest, SettleRequest, VoucherStatus,
};
use mezgeb_offline::{
    CUSTOMER_CAP_MINOR, GRACE_MICROS, Outcome, PublicKey, Receipt, SecretKey, Tier, TierLimits,
    Voucher, crypto::SIGNATURE_LEN,
};
use uuid::Uuid;

use super::books;
use super::ids::{Ids, Kind};

/// A microsecond day.
pub const DAY: i64 = 86_400_000_000;

/// A bug planted in the model, to show the checker catches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bug {
    /// A second receipt for a recorded counter is taken as a new one, not
    /// as a conflict (protocol §9 check 6).
    SkipCounterCheck,
    /// A receipt's link to its predecessor is not checked (§9 check 9).
    SkipChainCheck,
}

impl Bug {
    pub const fn name(self) -> &'static str {
        match self {
            Bug::SkipCounterCheck => "skip-counter-check",
            Bug::SkipChainCheck => "skip-chain-check",
        }
    }

    pub fn parse(text: &str) -> Option<Bug> {
        match text {
            "skip-counter-check" => Some(Bug::SkipCounterCheck),
            "skip-chain-check" => Some(Bug::SkipChainCheck),
            _ => None,
        }
    }
}

/// A client's offline settings and the ledger accounts the model needs.
#[derive(Debug, Clone)]
pub struct Setup {
    pub client: ClientId,
    pub ids: Ids,
    pub chart: Chart,
    pub fee_bps: u32,
    pub late_days: i64,
    /// Every tier's limits; `None` if it is disabled.
    pub tiers: BTreeMap<Tier, Option<TierLimits>>,
    pub fee_account: AccountId,
    pub loss_account: AccountId,
    pub issuer: SecretKey,
}

#[derive(Debug, Clone)]
pub struct MDevice {
    pub owner: AccountId,
    pub tier: Tier,
    pub key: PublicKey,
    pub status: DeviceStatus,
    pub next_counter: u64,
    pub valid_until: i64,
}

#[derive(Debug, Clone)]
pub struct MVoucher {
    pub voucher: Voucher,
    pub owner: AccountId,
    pub status: VoucherStatus,
    pub returned_at: Option<i64>,
    /// When the voucher ended: a receipt arriving later is past its late
    /// deadline.
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct MReceipt {
    pub voucher: VoucherId,
    pub digest: [u8; 32],
    pub prev_digest: [u8; 32],
    pub payee: AccountId,
    pub amount: i64,
    pub event_time: i64,
    pub outcome: Outcome,
    pub received_at: i64,
    pub parts: Parts,
    pub gap_skipped: bool,
    /// Paid by a dispute's resolution, which checks no links.
    pub by_dispute: bool,
}

/// A second, different receipt for a recorded counter.
#[derive(Debug, Clone)]
pub struct Copy {
    pub device: DeviceId,
    pub counter: u64,
    pub digest: [u8; 32],
    pub receipt: Receipt,
    pub received_at: i64,
    pub paid: bool,
}

#[derive(Debug, Clone)]
pub struct MShortfall {
    pub id: Uuid,
    pub owner: AccountId,
    pub device: DeviceId,
    pub counter: u64,
    pub amount: i64,
    pub repaid: bool,
}

#[derive(Debug, Clone)]
pub struct MDispute {
    pub device: DeviceId,
    pub voucher: VoucherId,
    pub reason: Outcome,
    pub counter: u64,
    pub resolution: Option<DisputeResolution>,
}

/// One receipt paid, from wherever: what the checker audits.
#[derive(Debug, Clone)]
pub struct Payment {
    pub device: DeviceId,
    pub counter: u64,
    pub digest: [u8; 32],
    pub voucher: VoucherId,
    pub parts: Parts,
    /// A conflicting copy, paid by a dispute.
    pub copy: bool,
}

/// What one upload did: the outcome, and the counters it settled (this
/// receipt's and those waiting for it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingested {
    pub outcome: Outcome,
    pub settled: Vec<u64>,
}

/// What one tick did, as the engine reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ticked {
    pub settled_past_gaps: Vec<(VoucherId, Vec<u64>)>,
    pub graced: Vec<VoucherId>,
    /// With whether an entry returned anything.
    pub returned: Vec<(VoucherId, bool)>,
    pub ended: Vec<VoucherId>,
}

/// A voucher to issue.
#[derive(Debug, Clone)]
pub struct IssueRequest {
    pub voucher: VoucherId,
    pub device: DeviceId,
    pub entry: EntryId,
    pub escrow: AccountId,
    pub amount: i64,
    pub last_counter: u64,
    pub now: i64,
}

pub struct Model {
    pub ledger: MemLedger,
    pub setup: Setup,
    pub devices: BTreeMap<DeviceId, MDevice>,
    pub vouchers: BTreeMap<VoucherId, MVoucher>,
    pub receipts: BTreeMap<(DeviceId, u64), MReceipt>,
    pub copies: Vec<Copy>,
    pub shortfalls: Vec<MShortfall>,
    pub disputes: Vec<MDispute>,
    /// Every payment of a receipt, in order.
    pub payments: Vec<Payment>,
    /// Every voucher status change, in order: what the checker audits.
    pub transitions: Vec<(VoucherId, VoucherStatus, VoucherStatus)>,
    /// Vouchers changed since the checker last looked.
    pub dirty: BTreeSet<VoucherId>,
    /// Vouchers not yet ended: the only ones a tick or an issue looks at.
    pub live: BTreeSet<VoucherId>,
    /// Signatures already verified, by the receipt's digest: a duplicate
    /// upload (half of them) skips the costly P-256 check. The device's key
    /// never changes, so the answer cannot.
    verified: std::collections::HashSet<([u8; 32], [u8; 64])>,
    pub bug: Option<Bug>,
    next_entry: u64,
}

fn etb(minor: i64) -> Money {
    Money::from_minor(minor, Currency::ETB)
}

fn paid(outcome: Outcome) -> bool {
    matches!(outcome, Outcome::Settled | Outcome::SettledLate)
}

/// The fee on `amount` at `bps`, rounded down (the engine's `fee_on`).
pub fn fee_on(amount: i64, bps: u32) -> i64 {
    i64::try_from(i128::from(amount) * i128::from(bps) / 10_000).unwrap_or(0)
}

/// What a dispute pays: a recorded receipt, or a conflicting copy.
struct Unpaid {
    counter: u64,
    received_at: i64,
    payee: AccountId,
    amount: i64,
    event_time: i64,
    copy: Option<usize>,
}

impl Model {
    pub fn new(setup: Setup) -> Model {
        Model {
            ledger: MemLedger::new(),
            setup,
            devices: BTreeMap::new(),
            vouchers: BTreeMap::new(),
            receipts: BTreeMap::new(),
            copies: Vec::new(),
            shortfalls: Vec::new(),
            disputes: Vec::new(),
            payments: Vec::new(),
            transitions: Vec::new(),
            dirty: BTreeSet::new(),
            live: BTreeSet::new(),
            verified: std::collections::HashSet::new(),
            bug: None,
            next_entry: 0,
        }
    }

    /// The model's next entry ID. The engine mints its own for the offline
    /// entries; only the IDs differ.
    pub fn fresh_entry(&mut self) -> Result<EntryId, String> {
        self.next_entry += 1;
        self.setup.ids.get(Kind::Entry, self.next_entry)
    }

    pub fn open(&mut self, id: AccountId, reference: &str) -> Result<(), LedgerError> {
        self.ledger.open_account(id, &self.setup.chart, reference)?;
        Ok(())
    }

    pub fn balance(&self, id: AccountId) -> i64 {
        self.ledger.account(id).map_or(0, |a| a.balance().amount())
    }

    fn post(&mut self, draft: EntryDraft) -> Result<EntryId, LedgerError> {
        let id = self.fresh_entry().map_err(LedgerError::Storage)?;
        self.ledger.post(id, draft)?;
        Ok(id)
    }

    /// An online posting, under its own ID.
    pub fn post_as(&mut self, id: EntryId, draft: EntryDraft) -> Result<(), LedgerError> {
        self.ledger.post(id, draft)?;
        Ok(())
    }

    fn set_voucher_status(&mut self, id: VoucherId, to: VoucherStatus) {
        if let Some(v) = self.vouchers.get_mut(&id) {
            self.transitions.push((id, v.status, to));
            v.status = to;
            self.dirty.insert(id);
            if to == VoucherStatus::Ended {
                self.live.remove(&id);
            }
        }
    }

    // Devices (protocol §5).

    /// Registers a device (`register_device`); the attestation is trusted.
    pub fn register(
        &mut self,
        id: DeviceId,
        owner: AccountId,
        tier: Tier,
        key: PublicKey,
        now: i64,
    ) -> Result<(), OfflineError> {
        match self.ledger.account(owner) {
            Some(a) if a.code().value() == 2010 && a.status().as_str() == "open" => {}
            _ => return Err(OfflineError::NotAWallet(owner)),
        }
        if self.setup.tiers.get(&tier).copied().flatten().is_none() {
            return Err(OfflineError::TierDisabled(tier));
        }
        self.devices.insert(
            id,
            MDevice {
                owner,
                tier,
                key,
                status: DeviceStatus::Active,
                next_counter: 1,
                valid_until: now + mezgeb_offline::CERTIFICATE_MICROS,
            },
        );
        Ok(())
    }

    /// Moves a device to `to` (`set_device_status`).
    pub fn set_device_status(
        &mut self,
        id: DeviceId,
        to: DeviceStatus,
    ) -> Result<(), OfflineError> {
        let device = self
            .devices
            .get_mut(&id)
            .ok_or(OfflineError::DeviceNotFound)?;
        if device.status == to {
            return Ok(());
        }
        if !device.status.may_become(to) {
            return Err(OfflineError::BadTransition {
                from: device.status,
                to,
            });
        }
        device.status = to;
        Ok(())
    }

    // Vouchers (protocol §7).

    /// Issues a voucher (`issue_offline_voucher`), with the checks in the
    /// engine's order.
    pub fn issue(&mut self, r: &IssueRequest) -> Result<Voucher, OfflineError> {
        let device = self
            .devices
            .get(&r.device)
            .ok_or(OfflineError::DeviceNotFound)?
            .clone();
        if device.status != DeviceStatus::Active {
            return Err(OfflineError::DeviceNotActive(device.status));
        }
        if device.valid_until <= r.now {
            return Err(OfflineError::CertificateExpired);
        }
        let limits = self
            .setup
            .tiers
            .get(&device.tier)
            .copied()
            .flatten()
            .ok_or(OfflineError::TierDisabled(device.tier))?;
        if r.amount > limits.max_voucher_minor {
            return Err(OfflineError::OverTierLimit {
                max_minor: limits.max_voucher_minor,
            });
        }
        let reported = i64::try_from(r.last_counter)
            .ok()
            .and_then(|c| c.checked_add(1))
            .ok_or_else(|| OfflineError::Invalid("the last counter is too large".into()))?;
        let recorded = self
            .receipts
            .range((r.device, 0)..=(r.device, u64::MAX))
            .next_back()
            .map_or(0, |((_, c), _)| *c);
        let counter_start = (reported as u64)
            .max(recorded.saturating_add(1))
            .max(device.next_counter);
        let open = |v: &MVoucher| {
            matches!(
                v.status,
                VoucherStatus::Active | VoucherStatus::Grace | VoucherStatus::Frozen
            )
        };
        let live = || self.live.iter().filter_map(|id| self.vouchers.get(id));
        if live().any(|v| v.voucher.device_id == r.device && open(v)) {
            return Err(OfflineError::VoucherOpen);
        }
        let outstanding: i64 = live()
            .filter(|v| v.owner == device.owner && open(v))
            .map(|v| v.voucher.amount.amount())
            .sum();
        let unpaid: i64 = self
            .shortfalls
            .iter()
            .filter(|s| s.owner == device.owner && !s.repaid)
            .map(|s| s.amount)
            .sum();
        if unpaid > 0 {
            return Err(OfflineError::ShortfallUnpaid {
                amount_minor: unpaid,
            });
        }
        if outstanding.saturating_add(r.amount) > CUSTOMER_CAP_MINOR {
            return Err(OfflineError::OverCustomerCap {
                outstanding_minor: outstanding,
                cap_minor: CUSTOMER_CAP_MINOR,
            });
        }
        // E16, checked before the escrow opens: the engine opens it in the
        // transaction that a refused posting rolls back.
        let reference = format!("2050:vch-{}", r.voucher);
        let escrow = Account::open(r.escrow, &self.setup.chart, &reference)
            .map_err(|e| OfflineError::Ledger(LedgerError::Chart(e)))?;
        let draft = books::issue(r.voucher, device.owner, r.escrow, etb(r.amount));
        let mut view = BTreeMap::new();
        if let Some(wallet) = self.ledger.account(device.owner) {
            view.insert(device.owner, wallet.clone());
        }
        view.insert(r.escrow, escrow);
        mezgeb_core::validate(draft.clone(), &view)
            .map_err(|e| OfflineError::Ledger(LedgerError::Posting(e)))?;
        self.ledger
            .open_account(r.escrow, &self.setup.chart, &reference)?;
        self.ledger.post(r.entry, draft)?;
        let voucher = Voucher {
            voucher_id: r.voucher,
            device_id: r.device,
            client_id: self.setup.client,
            escrow_account_id: r.escrow,
            amount: etb(r.amount),
            counter_start,
            issued_at: Timestamp::from_micros(r.now),
            expires_at: Timestamp::from_micros(r.now + limits.lifetime_micros),
            issuer_key_id: self.setup.issuer.public_key().key_id(),
        };
        self.live.insert(r.voucher);
        self.vouchers.insert(
            r.voucher,
            MVoucher {
                voucher: voucher.clone(),
                owner: device.owner,
                status: VoucherStatus::Active,
                returned_at: None,
                ended_at: None,
            },
        );
        self.dirty.insert(r.voucher);
        if let Some(d) = self.devices.get_mut(&r.device) {
            d.next_counter = counter_start;
        }
        Ok(voucher)
    }

    // Receipts (protocol §9).

    /// The payee must be an open wallet or merchant balance of the client.
    fn payee_ok(&self, payee: AccountId) -> bool {
        self.ledger.account(payee).is_some_and(|a| {
            matches!(a.code().value(), 2010 | 2030) && a.status().as_str() == "open"
        })
    }

    /// Uploads one receipt (`ingest_receipt`).
    pub fn ingest(&mut self, body: &[u8], signature: &[u8], now: i64) -> Ingested {
        let refused = |outcome| Ingested {
            outcome,
            settled: Vec::new(),
        };
        let Ok(receipt) = Receipt::decode(body) else {
            return refused(Outcome::BadFormat);
        };
        if signature.len() != SIGNATURE_LEN {
            return refused(Outcome::BadSignature);
        }
        let Some(device) = self.devices.get(&receipt.device_id) else {
            return refused(Outcome::UnknownDevice);
        };
        let digest = receipt.digest();
        let mut sig = [0u8; 64];
        sig.copy_from_slice(signature);
        if !self.verified.contains(&(digest, sig)) {
            if receipt.verify(&device.key, signature).is_err() {
                return refused(Outcome::BadSignature);
            }
            self.verified.insert((digest, sig));
        }
        let device_status = device.status;
        let Some(voucher) = self
            .vouchers
            .get(&receipt.voucher_id)
            .filter(|v| v.voucher.device_id == receipt.device_id)
        else {
            return refused(Outcome::UnknownVoucher);
        };
        if receipt.counter < voucher.voucher.counter_start {
            return refused(Outcome::UnknownVoucher);
        }
        if receipt.amount.currency() != voucher.voucher.amount.currency()
            || !receipt.amount.is_positive()
        {
            return refused(Outcome::BadFormat);
        }
        if !self.payee_ok(receipt.payee_account_id) {
            return refused(Outcome::BadFormat);
        }
        let voucher_status = voucher.status;
        let (d, c) = (receipt.device_id, receipt.counter);
        if let Some(stored) = self.receipts.get(&(d, c)) {
            if stored.digest == digest {
                return refused(Outcome::Duplicate);
            }
            if self.bug != Some(Bug::SkipCounterCheck) {
                if !self
                    .copies
                    .iter()
                    .any(|k| k.device == d && k.counter == c && k.digest == digest)
                {
                    self.copies.push(Copy {
                        device: d,
                        counter: c,
                        digest,
                        receipt: receipt.clone(),
                        received_at: now,
                        paid: false,
                    });
                }
                self.freeze(d, receipt.voucher_id, c, Outcome::Conflict);
                return refused(Outcome::Conflict);
            }
        }
        let frozen =
            device_status == DeviceStatus::Frozen || voucher_status == VoucherStatus::Frozen;
        self.receipts.insert(
            (d, c),
            MReceipt {
                voucher: receipt.voucher_id,
                digest,
                prev_digest: receipt.prev_digest,
                payee: receipt.payee_account_id,
                amount: receipt.amount.amount(),
                event_time: receipt.event_time.as_micros(),
                outcome: if frozen {
                    Outcome::Frozen
                } else {
                    Outcome::WaitingForGap
                },
                received_at: now,
                parts: Parts::default(),
                gap_skipped: false,
                by_dispute: false,
            },
        );
        self.dirty.insert(receipt.voucher_id);
        if let Some(device) = self.devices.get_mut(&d) {
            device.next_counter = device.next_counter.max(c.saturating_add(1));
        }
        if frozen {
            return refused(Outcome::Frozen);
        }
        let voucher_id = receipt.voucher_id;
        let outcome = self.decide(voucher_id, c, false, now);
        let mut settled = Vec::new();
        if paid(outcome) {
            settled.push(c);
            if self
                .receipts
                .get(&(d, c + 1))
                .is_some_and(|n| n.outcome == Outcome::WaitingForGap)
            {
                settled.extend(self.settle_waiting(voucher_id, false, now));
            }
        } else {
            self.apply_unpaid(voucher_id, c, outcome);
        }
        Ingested { outcome, settled }
    }

    /// Freezes the device and its voucher, and opens the dispute (§10).
    fn freeze(&mut self, device: DeviceId, voucher: VoucherId, counter: u64, reason: Outcome) {
        if let Some(d) = self.devices.get_mut(&device)
            && d.status == DeviceStatus::Active
        {
            d.status = DeviceStatus::Frozen;
        }
        if self
            .vouchers
            .get(&voucher)
            .is_some_and(|v| matches!(v.status, VoucherStatus::Active | VoucherStatus::Grace))
        {
            self.set_voucher_status(voucher, VoucherStatus::Frozen);
        }
        if !self
            .disputes
            .iter()
            .any(|x| x.voucher == voucher && x.resolution.is_none())
        {
            self.disputes.push(MDispute {
                device,
                voucher,
                reason,
                counter,
                resolution: None,
            });
        }
        self.dirty.insert(voucher);
    }

    /// The device's time, if it lies between the voucher's issue and now.
    fn bounded(v: &Voucher, time: i64, now: i64) -> Option<Timestamp> {
        (time >= v.issued_at.as_micros() && time <= now).then_some(Timestamp::from_micros(time))
    }

    /// Decides one recorded, unsettled receipt and pays it if it may be paid
    /// (`decide`).
    fn decide(&mut self, voucher_id: VoucherId, counter: u64, skip_gap: bool, now: i64) -> Outcome {
        let Some(v) = self.vouchers.get(&voucher_id).cloned() else {
            return Outcome::UnknownVoucher;
        };
        match v.status {
            VoucherStatus::Frozen => return Outcome::Frozen,
            VoucherStatus::Ended => return Outcome::Expired,
            _ => {}
        }
        let device = v.voucher.device_id;
        let Some(stored) = self.receipts.get(&(device, counter)).cloned() else {
            return Outcome::UnknownVoucher;
        };
        let successor = self
            .receipts
            .get(&(device, counter + 1))
            .filter(|s| s.voucher == voucher_id);
        let successor_paid = successor.is_some_and(|s| paid(s.outcome));
        if let Some(next) = successor.filter(|s| paid(s.outcome))
            && next.prev_digest != stored.digest
        {
            return Outcome::Conflict;
        }
        let late = matches!(v.status, VoucherStatus::Returned | VoucherStatus::Closed);
        let link = if counter == v.voucher.counter_start {
            Some(v.voucher.digest())
        } else {
            self.receipts
                .get(&(device, counter - 1))
                .filter(|p| paid(p.outcome))
                .map(|p| p.digest)
        };
        let mut gap_skipped = false;
        match link {
            Some(link) if link != stored.prev_digest && self.bug != Some(Bug::SkipChainCheck) => {
                return Outcome::Conflict;
            }
            Some(_) => {}
            None if successor_paid || skip_gap || late => gap_skipped = true,
            None => return Outcome::WaitingForGap,
        }
        if late {
            let parts = self.pay_receipt(
                &v,
                device,
                counter,
                stored.payee,
                stored.amount,
                stored.event_time,
                false,
                now,
                Some(stored.digest),
            );
            self.record_paid(device, counter, Outcome::SettledLate, parts, gap_skipped);
            return Outcome::SettledLate;
        }
        let left = self.balance(v.voucher.escrow_account_id);
        if stored.amount > left {
            return Outcome::OverLimit;
        }
        let fee = fee_on(stored.amount, self.setup.fee_bps);
        let event_time = Model::bounded(&v.voucher, stored.event_time, now);
        let request = SettleRequest {
            escrow: v.voucher.escrow_account_id,
            payee: stored.payee,
            fee_account: (fee > 0).then_some(self.setup.fee_account),
            amount: etb(stored.amount),
            fee: etb(fee),
            voucher: voucher_id,
            device,
            counter,
            event_time,
            claimed_time: event_time
                .is_none()
                .then_some(Timestamp::from_micros(stored.event_time)),
        };
        if let Err(e) = self.post(books::settle(&request)) {
            // The engine's posting would fail the same way: the model has
            // no answer for it.
            panic!("the model could not settle {device}:{counter}: {e}");
        }
        let parts = Parts {
            escrow: stored.amount,
            wallet: 0,
            loss: 0,
        };
        self.payments.push(Payment {
            device,
            counter,
            digest: stored.digest,
            voucher: voucher_id,
            parts,
            copy: false,
        });
        self.record_paid(device, counter, Outcome::Settled, parts, gap_skipped);
        Outcome::Settled
    }

    /// Pays a receipt by the loss policy: escrow (if `use_escrow`), then the
    /// payer's wallet, then the loss account, recording a shortfall
    /// (`pay_receipt`). `recorded` is the digest of a recorded receipt;
    /// `None` for a conflicting copy, recorded by the caller.
    #[allow(clippy::too_many_arguments)]
    fn pay_receipt(
        &mut self,
        v: &MVoucher,
        device: DeviceId,
        counter: u64,
        payee: AccountId,
        amount: i64,
        event_time: i64,
        use_escrow: bool,
        now: i64,
        recorded: Option<[u8; 32]>,
    ) -> Parts {
        let escrow = v.voucher.escrow_account_id;
        let from_escrow = if use_escrow {
            self.balance(escrow).clamp(0, amount)
        } else {
            0
        };
        let from_wallet = self.balance(v.owner).clamp(0, amount - from_escrow);
        let from_loss = amount - from_escrow - from_wallet;
        let fee = fee_on(amount, self.setup.fee_bps);
        let event = Model::bounded(&v.voucher, event_time, now);
        let request = PayRequest {
            escrow: (from_escrow > 0).then_some(escrow),
            wallet: (from_wallet > 0).then_some(v.owner),
            loss_account: (from_loss > 0).then_some(self.setup.loss_account),
            payee,
            fee_account: (fee > 0).then_some(self.setup.fee_account),
            from_escrow: etb(from_escrow),
            from_wallet: etb(from_wallet),
            from_loss: etb(from_loss),
            fee: etb(fee),
            voucher: v.voucher.voucher_id,
            device,
            counter,
            event_time: event,
            claimed_time: event
                .is_none()
                .then_some(Timestamp::from_micros(event_time)),
        };
        if let Err(e) = self.post(books::pay(&request)) {
            panic!("the model could not pay {device}:{counter}: {e}");
        }
        if from_loss > 0 {
            let n = self.shortfalls.len() as u64;
            let id = self
                .setup
                .ids
                .text(Kind::Entry, (1 << 23) | n)
                .parse()
                .unwrap_or_default();
            self.shortfalls.push(MShortfall {
                id,
                owner: v.owner,
                device,
                counter,
                amount: from_loss,
                repaid: false,
            });
        }
        let parts = Parts {
            escrow: from_escrow,
            wallet: from_wallet,
            loss: from_loss,
        };
        if let Some(digest) = recorded {
            self.payments.push(Payment {
                device,
                counter,
                digest,
                voucher: v.voucher.voucher_id,
                parts,
                copy: false,
            });
        }
        self.dirty.insert(v.voucher.voucher_id);
        parts
    }

    fn record_paid(
        &mut self,
        device: DeviceId,
        counter: u64,
        outcome: Outcome,
        parts: Parts,
        gap_skipped: bool,
    ) {
        if let Some(r) = self.receipts.get_mut(&(device, counter)) {
            r.outcome = outcome;
            r.parts = parts;
            r.gap_skipped = gap_skipped;
        }
    }

    /// A decision that pays nothing: a conflict or an over-spend freezes.
    fn apply_unpaid(&mut self, voucher: VoucherId, counter: u64, outcome: Outcome) {
        let Some(device) = self.vouchers.get(&voucher).map(|v| v.voucher.device_id) else {
            return;
        };
        if outcome != Outcome::WaitingForGap
            && let Some(r) = self.receipts.get_mut(&(device, counter))
        {
            r.outcome = outcome;
        }
        if matches!(outcome, Outcome::Conflict | Outcome::OverLimit) {
            self.freeze(device, voucher, counter, outcome);
        }
    }

    /// The voucher's receipts in counter order.
    fn of_voucher(&self, voucher: VoucherId) -> impl Iterator<Item = (u64, &MReceipt)> {
        let device = self
            .vouchers
            .get(&voucher)
            .map(|v| (v.voucher.device_id, v.voucher.counter_start));
        device
            .into_iter()
            .flat_map(move |(d, start)| self.receipts.range((d, start)..=(d, u64::MAX)))
            .filter(move |(_, r)| r.voucher == voucher)
            .map(|((_, c), r)| (*c, r))
    }

    /// Settles the voucher's waiting receipts in counter order, as far as
    /// they go (`settle_waiting`).
    fn settle_waiting(&mut self, voucher: VoucherId, skip_gap: bool, now: i64) -> Vec<u64> {
        let mut settled = Vec::new();
        loop {
            let next = self
                .of_voucher(voucher)
                .find(|(_, r)| r.outcome == Outcome::WaitingForGap)
                .map(|(c, _)| c);
            let Some(counter) = next else { break };
            match self.decide(voucher, counter, skip_gap, now) {
                o if paid(o) => settled.push(counter),
                Outcome::WaitingForGap => break,
                o => {
                    self.apply_unpaid(voucher, counter, o);
                    break;
                }
            }
        }
        settled
    }

    // Time (protocol §7, §11, §12).

    fn gap_timed_out(&self, voucher: VoucherId, now: i64) -> bool {
        self.of_voucher(voucher).any(|(_, r)| {
            r.outcome == Outcome::WaitingForGap && r.received_at + GAP_TIMEOUT_MICROS <= now
        })
    }

    fn due(&self, v: &MVoucher, now: i64) -> bool {
        let id = v.voucher.voucher_id;
        let expires = v.voucher.expires_at.as_micros();
        match v.status {
            VoucherStatus::Active if expires <= now => true,
            VoucherStatus::Grace if expires + GRACE_MICROS <= now => true,
            VoucherStatus::Returned | VoucherStatus::Closed => v
                .returned_at
                .is_some_and(|t| t + self.setup.late_days * DAY <= now),
            VoucherStatus::Active | VoucherStatus::Grace => self.gap_timed_out(id, now),
            _ => false,
        }
    }

    /// Returns the escrow left and marks the voucher `RETURNED`
    /// (`return_escrow`). Whether an entry was posted.
    fn return_escrow(&mut self, voucher: VoucherId, reason: &'static str, now: i64) -> bool {
        let Some(v) = self.vouchers.get(&voucher).cloned() else {
            return false;
        };
        let left = self.balance(v.voucher.escrow_account_id);
        let posted = left > 0;
        if posted {
            let request = ReturnRequest {
                escrow: v.voucher.escrow_account_id,
                wallet: v.owner,
                amount: etb(left),
                voucher,
                reason,
            };
            if let Err(e) = self.post(books::return_escrow(&request)) {
                panic!("the model could not return {voucher}: {e}");
            }
        }
        self.set_voucher_status(voucher, VoucherStatus::Returned);
        if let Some(v) = self.vouchers.get_mut(&voucher) {
            v.returned_at = Some(now);
        }
        posted
    }

    /// Moves every due voucher forward to `now` (`offline_tick`).
    pub fn tick(&mut self, now: i64) -> Ticked {
        let due: Vec<VoucherId> = self
            .live
            .iter()
            .filter_map(|id| self.vouchers.get(id))
            .filter(|v| self.due(v, now))
            .map(|v| v.voucher.voucher_id)
            .collect();
        let mut report = Ticked::default();
        for id in due {
            let status = |m: &Model| m.vouchers.get(&id).map(|v| v.status);
            let open = |s: Option<VoucherStatus>| {
                matches!(s, Some(VoucherStatus::Active | VoucherStatus::Grace))
            };
            if open(status(self)) && self.gap_timed_out(id, now) {
                let settled = self.settle_waiting(id, true, now);
                if !settled.is_empty() {
                    report.settled_past_gaps.push((id, settled));
                }
            }
            let Some(v) = self.vouchers.get(&id).cloned() else {
                continue;
            };
            let expires = v.voucher.expires_at.as_micros();
            if v.status == VoucherStatus::Active && expires <= now {
                self.set_voucher_status(id, VoucherStatus::Grace);
                report.graced.push(id);
            }
            if status(self) == Some(VoucherStatus::Grace) && expires + GRACE_MICROS <= now {
                let settled = self.settle_waiting(id, true, now);
                if !settled.is_empty() {
                    report.settled_past_gaps.push((id, settled));
                }
                if status(self) == Some(VoucherStatus::Grace) {
                    let posted = self.return_escrow(id, "expired", now);
                    report.returned.push((id, posted));
                }
            }
            if let Some(v) = self.vouchers.get(&id)
                && matches!(v.status, VoucherStatus::Returned | VoucherStatus::Closed)
                && v.returned_at
                    .is_some_and(|t| t + self.setup.late_days * DAY <= now)
            {
                self.set_voucher_status(id, VoucherStatus::Ended);
                if let Some(v) = self.vouchers.get_mut(&id) {
                    v.ended_at = Some(now);
                }
                report.ended.push(id);
            }
        }
        report
    }

    // Shortfalls and disputes (protocol §12, §14).

    /// Repays a shortfall from the payer's wallet (`repay_shortfall`).
    pub fn repay(&mut self, n: usize) -> Result<(), OfflineError> {
        let shortfall = self
            .shortfalls
            .get(n)
            .cloned()
            .ok_or(OfflineError::ShortfallNotFound)?;
        if shortfall.repaid {
            return Ok(());
        }
        let request = RepayRequest {
            wallet: shortfall.owner,
            loss_account: self.setup.loss_account,
            amount: etb(shortfall.amount),
            shortfall: shortfall.id,
        };
        self.post(books::repay(&request))?;
        if let Some(s) = self.shortfalls.get_mut(n) {
            s.repaid = true;
        }
        Ok(())
    }

    fn unpaid(&self, voucher: VoucherId) -> Vec<Unpaid> {
        let mut out: Vec<Unpaid> = self
            .of_voucher(voucher)
            .filter(|(_, r)| {
                matches!(
                    r.outcome,
                    Outcome::Frozen
                        | Outcome::OverLimit
                        | Outcome::Conflict
                        | Outcome::WaitingForGap
                )
            })
            .map(|(c, r)| Unpaid {
                counter: c,
                payee: r.payee,
                amount: r.amount,
                event_time: r.event_time,
                received_at: r.received_at,
                copy: None,
            })
            .collect();
        let device = self.vouchers.get(&voucher).map(|v| v.voucher.device_id);
        for (i, k) in self.copies.iter().enumerate() {
            if Some(k.device) == device && !k.paid && k.receipt.voucher_id == voucher {
                out.push(Unpaid {
                    counter: k.counter,
                    payee: k.receipt.payee_account_id,
                    amount: k.receipt.amount.amount(),
                    event_time: k.receipt.event_time.as_micros(),
                    received_at: k.received_at,
                    copy: Some(i),
                });
            }
        }
        out.sort_by_key(|u| (u.counter, u.copy));
        out
    }

    /// Ends a dispute (`resolve_dispute`).
    pub fn resolve(
        &mut self,
        n: usize,
        resolution: DisputeResolution,
        now: i64,
    ) -> Result<(), OfflineError> {
        let dispute = self
            .disputes
            .get(n)
            .cloned()
            .ok_or(OfflineError::DisputeNotFound)?;
        if let Some(done) = dispute.resolution {
            return if done == resolution {
                Ok(())
            } else {
                Err(OfflineError::DisputeResolved(done.as_str()))
            };
        }
        let v = self
            .vouchers
            .get(&dispute.voucher)
            .cloned()
            .ok_or(OfflineError::VoucherNotFound)?;
        let device = v.voucher.device_id;
        // What arrived after the voucher ended is past its late deadline:
        // never paid, its payee bears it (protocol §12).
        let too_late = |received_at: i64| v.ended_at.is_some_and(|e| received_at >= e);
        match resolution {
            DisputeResolution::Paid => {
                for item in self.unpaid(dispute.voucher) {
                    if too_late(item.received_at) {
                        if item.copy.is_none()
                            && let Some(r) = self.receipts.get_mut(&(device, item.counter))
                        {
                            r.outcome = Outcome::Expired;
                        }
                        continue;
                    }
                    let digest = match item.copy {
                        None => self.receipts.get(&(device, item.counter)).map(|r| r.digest),
                        Some(_) => None,
                    };
                    let parts = self.pay_receipt(
                        &v,
                        device,
                        item.counter,
                        item.payee,
                        item.amount,
                        item.event_time,
                        item.copy.is_none(),
                        now,
                        digest,
                    );
                    match item.copy {
                        None => {
                            let outcome = if parts.escrow == item.amount {
                                Outcome::Settled
                            } else {
                                Outcome::SettledLate
                            };
                            self.record_paid(device, item.counter, outcome, parts, false);
                            if let Some(r) = self.receipts.get_mut(&(device, item.counter)) {
                                r.by_dispute = true;
                            }
                        }
                        Some(i) => {
                            if let Some(k) = self.copies.get_mut(i) {
                                k.paid = true;
                                self.payments.push(Payment {
                                    device,
                                    counter: k.counter,
                                    digest: k.digest,
                                    voucher: dispute.voucher,
                                    parts,
                                    copy: true,
                                });
                            }
                        }
                    }
                }
            }
            DisputeResolution::Refused => {
                let counters: Vec<u64> = self
                    .unpaid(dispute.voucher)
                    .iter()
                    .filter(|u| u.copy.is_none())
                    .map(|u| u.counter)
                    .collect();
                for c in counters {
                    if let Some(r) = self.receipts.get_mut(&(device, c)) {
                        r.outcome = Outcome::Refused;
                    }
                }
            }
        }
        // Only an open voucher has escrow to return; a returned or ended one
        // keeps its state and its return time.
        if matches!(
            v.status,
            VoucherStatus::Active | VoucherStatus::Grace | VoucherStatus::Frozen
        ) {
            self.return_escrow(dispute.voucher, "dispute", now);
        }
        if let Some(d) = self.devices.get_mut(&device) {
            d.status = DeviceStatus::Revoked;
        }
        if let Some(d) = self.disputes.get_mut(n) {
            d.resolution = Some(resolution);
        }
        self.dirty.insert(dispute.voucher);
        self.unfreeze_others(device, dispute.voucher, now);
        Ok(())
    }

    /// Receipts of the device's other vouchers, kept `FROZEN` only because
    /// the device was frozen, are decided now that the dispute has ended:
    /// expired past their voucher's late deadline, otherwise in counter
    /// order as if just uploaded.
    fn unfreeze_others(&mut self, device: DeviceId, disputed: VoucherId, now: i64) {
        let others: Vec<(VoucherId, VoucherStatus)> = self
            .vouchers
            .values()
            .filter(|v| {
                v.voucher.device_id == device
                    && v.voucher.voucher_id != disputed
                    && v.status != VoucherStatus::Frozen
            })
            .map(|v| (v.voucher.voucher_id, v.status))
            .collect();
        for (voucher, status) in others {
            let frozen: Vec<u64> = self
                .of_voucher(voucher)
                .filter(|(_, r)| r.outcome == Outcome::Frozen)
                .map(|(c, _)| c)
                .collect();
            if frozen.is_empty() {
                continue;
            }
            let to = if status == VoucherStatus::Ended {
                Outcome::Expired
            } else {
                Outcome::WaitingForGap
            };
            for c in frozen {
                if let Some(r) = self.receipts.get_mut(&(device, c)) {
                    r.outcome = to;
                }
            }
            self.dirty.insert(voucher);
            if to == Outcome::WaitingForGap {
                self.settle_waiting(voucher, false, now);
            }
        }
    }

    /// The open disputes, by their index.
    pub fn open_disputes(&self) -> Vec<usize> {
        (0..self.disputes.len())
            .filter(|&i| self.disputes[i].resolution.is_none())
            .collect()
    }

    /// The unpaid shortfalls, by their index.
    pub fn unpaid_shortfalls(&self) -> Vec<usize> {
        (0..self.shortfalls.len())
            .filter(|&i| !self.shortfalls[i].repaid)
            .collect()
    }
}

/// A short name for an error, to compare the model's with the engine's.
pub fn error_kind(error: &OfflineError) -> String {
    let text = format!("{error:?}");
    match error {
        // The posting's own error says which rule and line refused it.
        OfflineError::Ledger(LedgerError::Posting(_)) => text,
        _ => text
            .split(['(', '{', ' '])
            .next()
            .unwrap_or_default()
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fee_rounds_down_as_the_engines() {
        assert_eq!(fee_on(6_000, 200), 120);
        assert_eq!(fee_on(99, 200), 1);
        assert_eq!(fee_on(49, 200), 0);
    }

    #[test]
    fn bugs_have_names() {
        assert_eq!(
            Bug::parse("skip-counter-check"),
            Some(Bug::SkipCounterCheck)
        );
        assert_eq!(Bug::parse("skip-chain-check"), Some(Bug::SkipChainCheck));
        assert_eq!(Bug::parse("other"), None);
    }
}
