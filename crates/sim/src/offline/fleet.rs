//! The simulated world of one lane and epoch: a client's customers and
//! their devices, its merchants, a ledger clock, and everything a real
//! fleet does to the ledger.
//!
//! Devices pay offline with real signed receipts. Each receipt is handed
//! to its payer and its payee, and either may upload it, at any time, in
//! any order, any number of times, or never. On top of that the world:
//!
//! - clones devices, and rolls phones back to a backup;
//! - lets a buggy device sign more than its voucher holds;
//! - forges and tampers with receipts, and replays old ones;
//! - skews device clocks, some wildly;
//! - lets time pass, so vouchers expire, return and end;
//! - drains wallets, so late receipts leave shortfalls; and has the
//!   operator resolve disputes and payers repay.
//!
//! [`World::next`] picks the next operation; operations that reach the
//! ledger are an [`Op`]; the ledger's answer comes back to
//! [`World::after`].

use std::collections::BTreeMap;

use mezgeb_core::{
    AccountId, Chart, ClientId, DeviceId, EntryDraft, EntryId, Timestamp, VoucherId,
};
use mezgeb_engine::{DeviceStatus, DisputeResolution, VoucherStatus};
use mezgeb_offline::{Receipt, SecretKey, Tier, TierLimits, Voucher};
use sha2::{Digest, Sha256};

use super::check::Truth;
use super::ids::{Ids, Kind};
use super::model::{DAY, IssueRequest, Model, Setup};
use crate::rng::SplitMix64;

pub const CHART: &str = r#"
name = "Offline simulation chart"
currency = "ETB"

[[accounts]]
code = 1010
name = "Lane banks"
type = "asset"
kind = "control"
prefix = "bank"
status = "active"

[[accounts]]
code = 2010
name = "Customer wallets"
type = "liability"
kind = "control"
prefix = "cust"
status = "active"

[[accounts]]
code = 2030
name = "Merchant balances"
type = "liability"
kind = "control"
prefix = "merch"
status = "active"

[[accounts]]
code = 2050
name = "Offline escrow"
type = "liability"
kind = "control"
prefix = "vch"
status = "active"

[[accounts]]
code = 4010
name = "Payment fee revenue"
type = "revenue"
kind = "control"
prefix = "fee"
status = "active"

[[accounts]]
code = 5030
name = "Offline settlement losses"
type = "expense"
kind = "control"
prefix = "loss"
status = "active"
"#;

/// 2026-11-16T00:00:00Z: every epoch starts here.
pub const START: i64 = 1_794_787_200_000_000;
const MINUTE: i64 = 60_000_000;
const HOUR: i64 = 60 * MINUTE;

/// Account numbers within a lane and epoch.
const BANK: u64 = 0;
const FEES: u64 = 1;
const LOSS: u64 = 2;
const WALLETS: u64 = 1_000;
const MERCHANTS: u64 = 100_000;
const ESCROWS: u64 = 200_000;

/// The shape of one lane's fleet.
#[derive(Debug, Clone, Copy)]
pub struct Shape {
    pub seed: u64,
    pub devices: u64,
}

/// One operation that reaches the ledger.
#[derive(Debug, Clone)]
pub enum Op {
    /// Opens an account at the start of an epoch.
    Open {
        id: AccountId,
        reference: String,
    },
    /// Registers a device at the start of an epoch.
    Register {
        device: DeviceId,
        owner: AccountId,
        tier: Tier,
        key: SecretKey,
    },
    /// An online posting between a wallet and the bank.
    Post {
        entry: EntryId,
        key: String,
        debit: AccountId,
        credit: AccountId,
        amount: i64,
    },
    Issue(IssueRequest),
    Upload {
        body: Vec<u8>,
        signature: Vec<u8>,
        now: i64,
    },
    Tick {
        now: i64,
    },
    /// The operator ends a dispute, by its index in the model.
    Resolve {
        dispute: usize,
        resolution: DisputeResolution,
        now: i64,
    },
    /// A payer repays a shortfall, by its index in the model.
    Repay {
        shortfall: usize,
    },
    /// The owner reports the device lost.
    Lost {
        device: DeviceId,
    },
}

impl Op {
    pub fn kind(&self) -> &'static str {
        match self {
            Op::Open { .. } => "open",
            Op::Register { .. } => "register",
            Op::Post { .. } => "post",
            Op::Issue(_) => "issue",
            Op::Upload { .. } => "upload",
            Op::Tick { .. } => "tick",
            Op::Resolve { .. } => "resolve",
            Op::Repay { .. } => "repay",
            Op::Lost { .. } => "lost",
        }
    }
}

/// What the model answered, in a form the engine's answer is compared to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Done(Result<(), String>),
    Issued(Result<Voucher, String>),
    Uploaded(super::model::Ingested),
    Ticked(super::model::Ticked),
}

/// A device as it is in someone's hand: its key, its voucher and its
/// counter. A clone is another `Dev` with the same ID and key.
#[derive(Debug, Clone)]
struct Dev {
    id: DeviceId,
    key: SecretKey,
    owner: usize,
    tier: Tier,
    clone: bool,
    voucher: Option<Voucher>,
    counter: u64,
    remaining: i64,
    last: [u8; 32],
    skew: i64,
    buggy: bool,
    backup: Option<(Option<Voucher>, u64, i64, [u8; 32])>,
}

/// A signed receipt in someone's hand, not yet uploaded.
#[derive(Debug, Clone)]
struct Delivery {
    body: Vec<u8>,
    signature: Vec<u8>,
}

pub struct World {
    pub ids: Ids,
    pub client: ClientId,
    pub now: i64,
    pub truth: Truth,
    pub setup: Setup,
    rng: SplitMix64,
    bank: AccountId,
    wallets: Vec<AccountId>,
    merchants: Vec<AccountId>,
    devs: Vec<Dev>,
    originals: usize,
    pending: Vec<Delivery>,
    archive: Vec<Delivery>,
    attacker: SecretKey,
    next_voucher: u64,
    next_online: u64,
    clones: usize,
}

/// The most receipts waiting to be uploaded in one lane; older ones are
/// lost.
const PENDING_MAX: usize = 4_000;
/// The most uploaded receipts kept for replays.
const ARCHIVE_MAX: usize = 256;

fn key_from(text: &str) -> Result<SecretKey, String> {
    SecretKey::from_bytes(&Sha256::digest(text.as_bytes()).into()).map_err(|e| e.to_string())
}

impl World {
    /// Lane `lane`'s world in epoch `epoch`.
    pub fn new(shape: Shape, lane: u64, epoch: u64) -> Result<World, String> {
        let ids = Ids::new(lane, epoch)?;
        let mut rng = SplitMix64::new(
            shape.seed
                ^ lane.wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ epoch.wrapping_mul(0xD1B5_4A32_D192_ED03),
        );
        let client: ClientId = ids.get(Kind::Client, 0)?;
        let account = |n: u64| ids.get::<AccountId>(Kind::Account, n);
        let customers = shape.devices.div_ceil(2).max(2);
        let merchants = (shape.devices / 8).max(2);
        let mut tiers = BTreeMap::new();
        for tier in Tier::ALL {
            // Some lanes lower a limit or disable tier C.
            let max = tier.maximum();
            let limits = match rng.below(10) {
                0 if tier == Tier::C => None,
                1 => Some(TierLimits {
                    max_voucher_minor: max.max_voucher_minor / 2,
                    lifetime_micros: max.lifetime_micros,
                }),
                _ => Some(max),
            };
            tiers.insert(tier, limits);
        }
        let late_days = if rng.below(2) == 0 {
            90
        } else {
            1 + rng.below(30) as i64
        };
        let setup = Setup {
            client,
            ids,
            chart: Chart::from_toml(CHART).map_err(|e| e.to_string())?,
            fee_bps: [0, 50, 150, 200, 1_000][rng.below(5) as usize],
            late_days,
            tiers,
            fee_account: account(FEES)?,
            loss_account: account(LOSS)?,
            issuer: key_from("mezgeb-sim issuer")?,
        };
        let mut devs = Vec::new();
        for n in 0..shape.devices {
            let tier = match rng.below(10) {
                0..4 => Tier::A,
                4..8 => Tier::B,
                _ => Tier::C,
            };
            let skew = match rng.below(10) {
                0..7 => rng.below(10 * MINUTE as u64) as i64 - 5 * MINUTE,
                7..9 => rng.below(2 * DAY as u64) as i64 - DAY,
                _ => rng.below(2 * 365 * DAY as u64) as i64 - 365 * DAY,
            };
            devs.push(Dev {
                id: ids.get(Kind::Device, n)?,
                key: key_from(&format!(
                    "mezgeb-sim {} {lane} {epoch} device {n}",
                    shape.seed
                ))?,
                owner: (n % customers) as usize,
                tier,
                clone: false,
                voucher: None,
                counter: 0,
                remaining: 0,
                last: [0; 32],
                skew,
                buggy: tier != Tier::A && rng.below(20) == 0,
                backup: None,
            });
        }
        Ok(World {
            ids,
            client,
            now: START,
            truth: Truth::default(),
            setup,
            rng,
            bank: account(BANK)?,
            wallets: (0..customers)
                .map(|n| account(WALLETS + n))
                .collect::<Result<_, _>>()?,
            merchants: (0..merchants)
                .map(|n| account(MERCHANTS + n))
                .collect::<Result<_, _>>()?,
            originals: devs.len(),
            devs,
            pending: Vec::new(),
            archive: Vec::new(),
            attacker: key_from(&format!(
                "mezgeb-sim {} {lane} {epoch} attacker",
                shape.seed
            ))?,
            next_voucher: 0,
            next_online: 0,
            clones: 0,
        })
    }

    fn tag(&self) -> String {
        format!("l{}e{}", self.ids.lane, self.ids.epoch)
    }

    /// The operations that set an epoch up: its accounts, its devices, and
    /// money in every wallet.
    pub fn setup_ops(&mut self) -> Result<Vec<Op>, String> {
        let tag = self.tag();
        let mut ops = vec![
            Op::Open {
                id: self.bank,
                reference: format!("1010:bank-{tag}"),
            },
            Op::Open {
                id: self.setup.fee_account,
                reference: format!("4010:fee-{tag}"),
            },
            Op::Open {
                id: self.setup.loss_account,
                reference: format!("5030:loss-{tag}"),
            },
        ];
        for (n, id) in self.wallets.iter().enumerate() {
            ops.push(Op::Open {
                id: *id,
                reference: format!("2010:cust-{tag}-{n}"),
            });
        }
        for (n, id) in self.merchants.iter().enumerate() {
            ops.push(Op::Open {
                id: *id,
                reference: format!("2030:merch-{tag}-{n}"),
            });
        }
        for d in &self.devs {
            ops.push(Op::Register {
                device: d.id,
                owner: self.wallets[d.owner],
                tier: d.tier,
                key: d.key.clone(),
            });
        }
        for n in 0..self.wallets.len() {
            let amount = 1_000_000 + self.rng.below(4_000_000) as i64;
            ops.push(self.online(self.bank, self.wallets[n], amount)?);
        }
        Ok(ops)
    }

    fn online(&mut self, debit: AccountId, credit: AccountId, amount: i64) -> Result<Op, String> {
        self.next_online += 1;
        Ok(Op::Post {
            entry: self.ids.get(Kind::Entry, (1 << 22) | self.next_online)?,
            key: format!("sim-{}-{}", self.tag(), self.next_online),
            debit,
            credit,
            amount,
        })
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> Option<T> {
        (!items.is_empty()).then(|| items[self.rng.below(items.len() as u64) as usize])
    }

    /// A payee: usually a merchant, sometimes another customer.
    fn payee(&mut self) -> AccountId {
        if self.rng.below(10) < 7 {
            self.merchants[self.rng.below(self.merchants.len() as u64) as usize]
        } else {
            self.wallets[self.rng.below(self.wallets.len() as u64) as usize]
        }
    }

    /// Device `d` signs a receipt for `amount` to `payee`.
    fn sign(&mut self, d: usize, payee: AccountId, amount: i64) -> Option<Delivery> {
        let challenge = {
            let a = self.rng.next_u64().to_be_bytes();
            let b = self.rng.next_u64().to_be_bytes();
            let mut c = [0u8; 16];
            c[..8].copy_from_slice(&a);
            c[8..].copy_from_slice(&b);
            c
        };
        let payee_device = (self.rng.below(2) == 0).then(|| self.devs[0].id);
        let now = self.now;
        let dev = &mut self.devs[d];
        let voucher = dev.voucher.as_ref()?;
        dev.counter += 1;
        let receipt = Receipt {
            device_id: dev.id,
            voucher_id: voucher.voucher_id,
            counter: dev.counter,
            prev_digest: dev.last,
            payee_account_id: payee,
            payee_device_id: payee_device,
            amount: mezgeb_core::Money::from_minor(amount, mezgeb_core::Currency::ETB),
            event_time: Timestamp::from_micros(now + dev.skew),
            payee_challenge: challenge,
        };
        dev.last = receipt.digest();
        dev.remaining -= amount;
        let signature = receipt.sign(&dev.key).to_vec();
        self.truth.signed.insert(receipt.digest());
        Some(Delivery {
            body: receipt.body(),
            signature,
        })
    }

    /// Hands a receipt to its payer and its payee.
    fn deliver(&mut self, delivery: Delivery) {
        self.pending.push(delivery.clone());
        self.pending.push(delivery);
        if self.pending.len() > PENDING_MAX {
            self.pending.drain(..self.pending.len() - PENDING_MAX);
        }
    }

    fn upload(&mut self, delivery: Delivery) -> Op {
        if self.archive.len() >= ARCHIVE_MAX {
            let i = self.rng.below(ARCHIVE_MAX as u64) as usize;
            self.archive[i] = delivery.clone();
        } else {
            self.archive.push(delivery.clone());
        }
        Op::Upload {
            body: delivery.body,
            signature: delivery.signature,
            now: self.now,
        }
    }

    /// Devices with a voucher and something left on it.
    fn spenders(&self) -> Vec<usize> {
        (0..self.devs.len())
            .filter(|&i| self.devs[i].voucher.is_some() && self.devs[i].remaining > 0)
            .collect()
    }

    /// The next thing that happens: an operation on the ledger, or `None`
    /// for one that stays on devices (a payment, a clone, a lost receipt).
    #[allow(clippy::too_many_lines)]
    pub fn next(&mut self, model: &Model) -> Result<(&'static str, Option<Op>), String> {
        let pick = self.rng.below(1_000);
        Ok(match pick {
            // Money in.
            0..40 => {
                let n = self.rng.below(self.wallets.len() as u64) as usize;
                let amount = 1_000 + self.rng.below(2_000_000) as i64;
                (
                    "deposit",
                    Some(self.online(self.bank, self.wallets[n], amount)?),
                )
            }
            // Money out, sometimes all of it, so late receipts fall short.
            40..65 => {
                let n = self.rng.below(self.wallets.len() as u64) as usize;
                let balance = model.balance(self.wallets[n]);
                if balance <= 0 {
                    ("spend", None)
                } else {
                    let amount = if self.rng.below(2) == 0 {
                        balance
                    } else {
                        1 + self.rng.below(balance as u64) as i64
                    };
                    (
                        "spend",
                        Some(self.online(self.wallets[n], self.bank, amount)?),
                    )
                }
            }
            // A voucher for a device, sometimes above its tier's limit.
            65..155 => {
                // Usually a device that can take one; sometimes any.
                let ready: Vec<usize> =
                    (0..self.originals)
                        .filter(|&i| {
                            let id = self.devs[i].id;
                            model
                                .devices
                                .get(&id)
                                .is_some_and(|d| d.status == DeviceStatus::Active)
                                && !model.live.iter().filter_map(|v| model.vouchers.get(v)).any(
                                    |v| {
                                        v.voucher.device_id == id
                                            && matches!(
                                                v.status,
                                                VoucherStatus::Active
                                                    | VoucherStatus::Grace
                                                    | VoucherStatus::Frozen
                                            )
                                    },
                                )
                        })
                        .collect();
                let d = match self.pick(&ready) {
                    Some(d) if self.rng.below(5) != 0 => d,
                    _ => self.rng.below(self.originals as u64) as usize,
                };
                let max = self.devs[d].tier.maximum().max_voucher_minor;
                let amount = 1 + self.rng.below((max as u64) * 11 / 10) as i64;
                self.next_voucher += 1;
                let n = self.next_voucher;
                let request = IssueRequest {
                    voucher: self.ids.get(Kind::Voucher, n)?,
                    device: self.devs[d].id,
                    entry: self.ids.get(Kind::Entry, (1 << 21) | n)?,
                    escrow: self.ids.get(Kind::Account, ESCROWS + n)?,
                    amount,
                    last_counter: self.devs[d].counter,
                    now: self.now,
                };
                ("issue", Some(Op::Issue(request)))
            }
            // An offline payment.
            155..455 => {
                let spenders = self.spenders();
                match self.pick(&spenders) {
                    None => ("pay", None),
                    Some(d) => {
                        let left = self.devs[d].remaining;
                        let amount = 1 + self.rng.below(left.min(30_000) as u64) as i64;
                        let payee = self.payee();
                        if let Some(delivery) = self.sign(d, payee, amount) {
                            self.deliver(delivery);
                        }
                        ("pay", None)
                    }
                }
            }
            // The payer or the payee uploads a receipt; sometimes keeps a
            // copy and uploads it again later.
            455..785 => {
                if self.pending.is_empty() {
                    ("upload", None)
                } else {
                    // Usually one of the latest; sometimes one long held.
                    let n = self.pending.len() as u64;
                    let i = if self.rng.below(10) < 7 {
                        (n - 1 - self.rng.below(n.min(16))) as usize
                    } else {
                        self.rng.below(n) as usize
                    };
                    let delivery = if self.rng.below(10) < 8 {
                        self.pending.swap_remove(i)
                    } else {
                        self.pending[i].clone()
                    };
                    ("upload", Some(self.upload(delivery)))
                }
            }
            // An old receipt replayed.
            785..810 => match self.archive.is_empty() {
                true => ("replay", None),
                false => {
                    let i = self.rng.below(self.archive.len() as u64) as usize;
                    let d = self.archive[i].clone();
                    ("replay", Some(self.upload(d)))
                }
            },
            810..840 => ("forge", Some(self.forge()?)),
            // A device copied: both copies spend.
            840..844 => {
                let spenders: Vec<usize> = self
                    .spenders()
                    .into_iter()
                    .filter(|&i| !self.devs[i].clone)
                    .collect();
                if let Some(d) = self.pick(&spenders)
                    && self.clones < self.originals / 8 + 1
                {
                    let mut copy = self.devs[d].clone();
                    copy.clone = true;
                    self.devs.push(copy);
                    self.clones += 1;
                }
                ("clone", None)
            }
            // A phone backed up, and later restored from the backup.
            844..850 => {
                let d = self.rng.below(self.originals as u64) as usize;
                let dev = &mut self.devs[d];
                dev.backup = Some((dev.voucher.clone(), dev.counter, dev.remaining, dev.last));
                ("backup", None)
            }
            850..854 => {
                let d = self.rng.below(self.originals as u64) as usize;
                let dev = &mut self.devs[d];
                if dev.tier != Tier::A
                    && let Some((voucher, counter, remaining, last)) = dev.backup.clone()
                    && voucher.as_ref().map(|v| v.voucher_id)
                        == dev.voucher.as_ref().map(|v| v.voucher_id)
                {
                    dev.counter = counter;
                    dev.remaining = remaining;
                    dev.last = last;
                }
                ("restore", None)
            }
            // A buggy phone signs more than its voucher holds.
            854..860 => {
                let buggy: Vec<usize> = (0..self.devs.len())
                    .filter(|&i| self.devs[i].buggy && self.devs[i].voucher.is_some())
                    .collect();
                if let Some(d) = self.pick(&buggy) {
                    let amount = self.devs[d].remaining.max(0) + 1 + self.rng.below(5_000) as i64;
                    let payee = self.payee();
                    if let Some(delivery) = self.sign(d, payee, amount) {
                        self.deliver(delivery);
                    }
                }
                ("overspend", None)
            }
            // A receipt lost by whoever held it.
            860..875 => {
                if !self.pending.is_empty() {
                    let i = self.rng.below(self.pending.len() as u64) as usize;
                    self.pending.swap_remove(i);
                }
                ("lose", None)
            }
            875..878 => {
                let d = self.rng.below(self.originals as u64) as usize;
                (
                    "lost",
                    Some(Op::Lost {
                        device: self.devs[d].id,
                    }),
                )
            }
            // Time passes.
            878..968 => {
                self.now += match self.rng.below(20) {
                    0..14 => MINUTE + self.rng.below(29 * MINUTE as u64) as i64,
                    14..19 => HOUR + self.rng.below(5 * HOUR as u64) as i64,
                    _ => DAY + self.rng.below(DAY as u64) as i64,
                };
                ("tick", Some(Op::Tick { now: self.now }))
            }
            968..988 => match self.pick(&model.open_disputes()) {
                None => ("resolve", None),
                Some(dispute) => {
                    let resolution = if self.rng.below(10) < 7 {
                        DisputeResolution::Paid
                    } else {
                        DisputeResolution::Refused
                    };
                    (
                        "resolve",
                        Some(Op::Resolve {
                            dispute,
                            resolution,
                            now: self.now,
                        }),
                    )
                }
            },
            _ => match self.pick(&model.unpaid_shortfalls()) {
                None => ("repay", None),
                Some(shortfall) => ("repay", Some(Op::Repay { shortfall })),
            },
        })
    }

    /// A forged, tampered or misdirected receipt.
    fn forge(&mut self) -> Result<Op, String> {
        let with_voucher: Vec<usize> = (0..self.originals)
            .filter(|&i| self.devs[i].voucher.is_some())
            .collect();
        let base = self.archive.first().cloned();
        let op = |body: Vec<u8>, signature: Vec<u8>, now| Op::Upload {
            body,
            signature,
            now,
        };
        let kind = self.rng.below(8);
        let now = self.now;
        Ok(match (kind, base, self.pick(&with_voucher)) {
            // A real receipt with its amount changed.
            (0, Some(d), _) => match Receipt::decode(&d.body) {
                Ok(mut r) => {
                    r.amount = mezgeb_core::Money::from_minor(
                        r.amount.amount() + 1 + self.rng.below(1_000) as i64,
                        r.amount.currency(),
                    );
                    op(r.body(), d.signature, now)
                }
                Err(_) => op(d.body, d.signature, now),
            },
            // Signed by an attacker's key in a real device's name.
            (1, Some(d), _) => match Receipt::decode(&d.body) {
                Ok(mut r) => {
                    r.payee_account_id = self.payee();
                    let signature = r.sign(&self.attacker).to_vec();
                    op(r.body(), signature, now)
                }
                Err(_) => op(d.body, d.signature, now),
            },
            // A signature of the wrong length.
            (2, Some(d), _) => op(d.body, d.signature[..32].to_vec(), now),
            // A device the ledger never registered.
            (3, Some(d), _) => match Receipt::decode(&d.body) {
                Ok(mut r) => {
                    r.device_id = self.ids.get(Kind::Device, 0x0f_ffff)?;
                    let signature = r.sign(&self.attacker).to_vec();
                    op(r.body(), signature, now)
                }
                Err(_) => op(d.body, d.signature, now),
            },
            // A real device names another device's voucher.
            (4, _, Some(d)) => {
                let other = self.rng.below(self.originals as u64) as usize;
                let unknown: VoucherId = self.ids.get(Kind::Voucher, super::ids::MAX_N)?;
                let voucher = self.devs[other]
                    .voucher
                    .as_ref()
                    .filter(|v| v.device_id != self.devs[d].id)
                    .map_or(unknown, |v| v.voucher_id);
                self.misuse(d, |r| r.voucher_id = voucher)
            }
            // A real device signs a counter before its voucher's first.
            (5, _, Some(d)) => self.misuse(d, |r| r.counter = 0),
            // A real device pays an account that is not a payee.
            (6, _, Some(d)) => {
                let bank = self.bank;
                self.misuse(d, |r| r.payee_account_id = bank)
            }
            // Bytes that are not a receipt at all.
            _ => {
                let mut body = vec![0u8; 1 + self.rng.below(200) as usize];
                for b in &mut body {
                    *b = self.rng.below(256) as u8;
                }
                op(body, vec![0; 64], now)
            }
        })
    }

    /// A receipt that device `d` (which has a voucher) signs with its real
    /// key, changed by `change`.
    fn misuse(&mut self, d: usize, change: impl Fn(&mut Receipt)) -> Op {
        let amount = 1 + self.rng.below(1_000) as i64;
        let dev = &self.devs[d];
        let Some(voucher_id) = dev.voucher.as_ref().map(|v| v.voucher_id) else {
            return Op::Tick { now: self.now };
        };
        let mut r = Receipt {
            device_id: dev.id,
            voucher_id,
            counter: dev.counter + 1,
            prev_digest: dev.last,
            payee_account_id: self.merchants[0],
            payee_device_id: None,
            amount: mezgeb_core::Money::from_minor(amount, mezgeb_core::Currency::ETB),
            event_time: Timestamp::from_micros(self.now + dev.skew),
            payee_challenge: [7; 16],
        };
        change(&mut r);
        let signature = r.sign(&dev.key).to_vec();
        self.truth.signed.insert(r.digest());
        Op::Upload {
            body: r.body(),
            signature,
            now: self.now,
        }
    }

    /// The ledger's answer to an operation, as far as devices learn it.
    pub fn after(&mut self, op: &Op, answer: &Answer) {
        if let (Op::Issue(request), Answer::Issued(Ok(voucher))) = (op, answer)
            && let Some(dev) = self
                .devs
                .iter_mut()
                .find(|d| d.id == request.device && !d.clone)
        {
            dev.counter = voucher.counter_start - 1;
            dev.remaining = voucher.amount.amount();
            dev.last = voucher.digest();
            dev.voucher = Some(voucher.clone());
        }
    }

    /// At the end of an epoch, the next receipt still held, uploaded.
    pub fn drain_one(&mut self) -> Option<Op> {
        if self.pending.is_empty() {
            return None;
        }
        let i = self.rng.below(self.pending.len() as u64) as usize;
        let d = self.pending.swap_remove(i);
        Some(Op::Upload {
            body: d.body,
            signature: d.signature,
            now: self.now,
        })
    }

    /// A tick far enough ahead for everything due to fall due.
    pub fn far_tick(&mut self) -> Op {
        self.now += 400 * DAY;
        Op::Tick { now: self.now }
    }
}

/// Applies an operation to the model.
pub fn apply(model: &mut Model, op: &Op) -> Answer {
    use super::model::error_kind;
    let done =
        |r: Result<(), mezgeb_engine::OfflineError>| Answer::Done(r.map_err(|e| error_kind(&e)));
    match op {
        Op::Open { id, reference } => {
            Answer::Done(model.open(*id, reference).map_err(|e| format!("{e:?}")))
        }
        Op::Register {
            device,
            owner,
            tier,
            key,
        } => done(model.register(*device, *owner, *tier, key.public_key().clone(), START)),
        Op::Post {
            entry,
            debit,
            credit,
            amount,
            ..
        } => {
            let money = mezgeb_core::Money::from_minor(*amount, mezgeb_core::Currency::ETB);
            let draft = EntryDraft::default()
                .debit(*debit, money)
                .credit(*credit, money);
            Answer::Done(model.post_as(*entry, draft).map_err(|e| format!("{e:?}")))
        }
        Op::Issue(request) => Answer::Issued(model.issue(request).map_err(|e| error_kind(&e))),
        Op::Upload {
            body,
            signature,
            now,
        } => Answer::Uploaded(model.ingest(body, signature, *now)),
        Op::Tick { now } => Answer::Ticked(model.tick(*now)),
        Op::Resolve {
            dispute,
            resolution,
            now,
        } => done(model.resolve(*dispute, *resolution, *now)),
        Op::Repay { shortfall } => done(model.repay(*shortfall)),
        Op::Lost { device } => done(model.set_device_status(*device, DeviceStatus::Lost)),
    }
}
