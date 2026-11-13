//! The workload: random operations in one lane, and the reference ledger
//! that says what each must do.
//!
//! A lane is a self-contained ledger: its own bank account, wallets, holds
//! and clock, and its own reference ledger ([`MemLedger`]). Nothing in one
//! lane touches another's accounts, so lanes run in parallel against one
//! database and each still has exactly one correct outcome per operation.

use std::collections::BTreeMap;

use mezgeb_core::{
    AccountId, ActorId, Authorization, Chart, ClientId, Currency, EntryDraft, EntryId, EntryKind,
    Hold, HoldId, HoldStatus, LedgerError, MemLedger, Money, PositiveAmount, PostedEntry,
    PostedLine, ReasonCode, Timestamp, check_invariants,
};

use crate::rng::SplitMix64;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

pub const CHART: &str = r#"
name = "Simulation chart"
currency = "ETB"

[[accounts]]
code = 1020
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
code = 2020
name = "Held funds"
type = "liability"
kind = "control"
prefix = "hold"
status = "active"
"#;

/// Wallets per lane.
pub const WALLETS: u64 = 6;

/// The parts of an ID: lane, kind and number, packed into the last 48 bits
/// of a UUIDv7, so every ID says which lane it belongs to.
fn id_text(lane: u64, kind: u64, n: u64) -> String {
    let packed = (lane << 40) | (kind << 36) | n;
    format!("00000000-0000-7000-8000-{packed:012x}")
}

const ACCOUNT: u64 = 1;
const ENTRY: u64 = 2;
const HOLD: u64 = 3;
const CLIENT: u64 = 4;

/// One operation, as a client would send it.
#[derive(Debug, Clone)]
pub enum Op {
    /// Money arrives from the bank into a wallet.
    Deposit {
        wallet: u64,
        amount: i64,
    },
    /// A payment between two wallets; may overdraw and be refused.
    Transfer {
        from: u64,
        to: u64,
        amount: i64,
    },
    /// A compensation between wallets, approved by a second person, or
    /// (to be refused) by the same one.
    Adjust {
        from: u64,
        to: u64,
        amount: i64,
        self_approved: bool,
    },
    /// Reverses an earlier entry of this lane.
    Reverse {
        target: EntryId,
    },
    /// Places a hold on a wallet, in a held account opened for it.
    Place {
        hold: HoldId,
        wallet: u64,
        held: AccountId,
        amount: i64,
        expires_at: Timestamp,
    },
    /// Captures part or all of a hold into the bank or a wallet.
    Capture {
        hold: HoldId,
        destination: AccountId,
        amount: i64,
        now: Timestamp,
    },
    Release {
        hold: HoldId,
    },
    Expire {
        hold: HoldId,
        now: Timestamp,
    },
    /// Time passes; nothing is sent.
    Tick,
}

impl Op {
    pub const KINDS: [&'static str; 9] = [
        "deposit", "transfer", "adjust", "reverse", "place", "capture", "release", "expire", "tick",
    ];

    pub fn kind(&self) -> &'static str {
        match self {
            Op::Deposit { .. } => "deposit",
            Op::Transfer { .. } => "transfer",
            Op::Adjust { .. } => "adjust",
            Op::Reverse { .. } => "reverse",
            Op::Place { .. } => "place",
            Op::Capture { .. } => "capture",
            Op::Release { .. } => "release",
            Op::Expire { .. } => "expire",
            Op::Tick => "tick",
        }
    }
}

/// What an entry looks like, without its sequence number: the database
/// numbers entries across all lanes, the reference ledger within one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    pub id: EntryId,
    pub lines: Vec<PostedLine>,
    pub reverses: Option<EntryId>,
    pub kind: EntryKind,
    pub metadata: BTreeMap<String, String>,
    pub authorization: Option<Authorization>,
}

impl Shape {
    pub fn of(entry: &PostedEntry) -> Shape {
        Shape {
            id: entry.id(),
            lines: entry.lines().to_vec(),
            reverses: entry.reverses(),
            kind: entry.kind(),
            metadata: entry.metadata().clone(),
            authorization: entry.authorization().cloned(),
        }
    }
}

/// The result of one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Entry(Shape),
    Hold(Hold),
    Refused(LedgerError),
    Nothing,
}

impl Outcome {
    /// The entry this outcome wrote, if any.
    pub fn entry(&self, op: &Op) -> Option<EntryId> {
        match (self, op) {
            (Outcome::Entry(shape), _) => Some(shape.id),
            (Outcome::Hold(hold), Op::Place { .. }) => Some(hold.place_entry()),
            (Outcome::Hold(hold), _) => hold.close_entry(),
            _ => None,
        }
    }
}

pub struct Lane {
    pub number: u64,
    pub client: ClientId,
    pub oracle: MemLedger,
    pub chart: Chart,
    rng: SplitMix64,
    clock: i64,
    next_entry: u64,
    holds: Vec<HoldId>,
}

fn etb(amount: i64) -> Money {
    Money::from_minor(amount, Currency::ETB)
}

fn positive(amount: i64) -> Result<PositiveAmount, Error> {
    Ok(PositiveAmount::new(etb(amount))?)
}

fn actor(name: &str) -> Result<ActorId, Error> {
    Ok(ActorId::new(name)?)
}

impl Lane {
    /// Lane `number` of a run with this seed, its accounts open in the
    /// reference ledger.
    pub fn new(number: u64, seed: u64) -> Result<Lane, Error> {
        let mut lane = Lane {
            number,
            client: id_text(number, CLIENT, 0).parse()?,
            oracle: MemLedger::new(),
            chart: Chart::from_toml(CHART)?,
            rng: SplitMix64::new(seed ^ number.wrapping_mul(0x9E37_79B9_7F4A_7C15)),
            clock: 0,
            next_entry: 0,
            holds: Vec::new(),
        };
        for (id, reference) in lane.accounts()? {
            lane.oracle.open_account(id, &lane.chart, &reference)?;
        }
        Ok(lane)
    }

    pub fn bank(&self) -> Result<AccountId, Error> {
        Ok(id_text(self.number, ACCOUNT, 0).parse()?)
    }

    pub fn wallet(&self, n: u64) -> Result<AccountId, Error> {
        Ok(id_text(self.number, ACCOUNT, 1 + n).parse()?)
    }

    fn held(&self, k: u64) -> Result<(AccountId, String), Error> {
        Ok((
            id_text(self.number, ACCOUNT, 1_000 + k).parse()?,
            format!("2020:hold-l{}-{k}", self.number),
        ))
    }

    /// The bank and the wallets, with their chart references.
    pub fn accounts(&self) -> Result<Vec<(AccountId, String)>, Error> {
        let mut accounts = vec![(self.bank()?, format!("1020:bank-l{}", self.number))];
        for n in 0..WALLETS {
            accounts.push((self.wallet(n)?, format!("2010:cust-l{}-{n}", self.number)));
        }
        Ok(accounts)
    }

    /// A new entry ID in this lane, for each attempt.
    pub fn fresh_entry(&mut self) -> Result<EntryId, Error> {
        self.next_entry += 1;
        Ok(id_text(self.number, ENTRY, self.next_entry).parse()?)
    }

    /// The lane's random stream, also used for its faults.
    pub fn rng(&mut self) -> &mut SplitMix64 {
        &mut self.rng
    }

    /// The next operation, and, for a hold, the held account to open first.
    pub fn next_op(&mut self) -> Result<(Op, Option<(AccountId, String)>), Error> {
        let pick = self.rng.below(100);
        let wallet = self.rng.below(WALLETS);
        let other = (wallet + 1 + self.rng.below(WALLETS - 1)) % WALLETS;
        let small = 1 + self.rng.below(3_000) as i64;
        let now = Timestamp::from_micros(self.clock);
        let some_hold = (!self.holds.is_empty())
            .then(|| self.holds[self.rng.below(self.holds.len() as u64) as usize]);
        let op = match pick {
            0..20 => Op::Deposit {
                wallet,
                amount: 1 + self.rng.below(10_000) as i64,
            },
            20..50 => Op::Transfer {
                from: wallet,
                to: other,
                amount: small,
            },
            50..55 => Op::Adjust {
                from: wallet,
                to: other,
                amount: small,
                self_approved: self.rng.below(5) == 0,
            },
            55..62 => match self.oracle.entries() {
                [] => Op::Tick,
                entries => Op::Reverse {
                    target: entries[self.rng.below(entries.len() as u64) as usize].id(),
                },
            },
            62..75 => {
                let k = self.holds.len() as u64;
                let hold: HoldId = id_text(self.number, HOLD, k).parse()?;
                let (held, reference) = self.held(k)?;
                self.holds.push(hold);
                let op = Op::Place {
                    hold,
                    wallet,
                    held,
                    amount: 1 + self.rng.below(2_000) as i64,
                    expires_at: Timestamp::from_micros(self.clock + 1 + self.rng.below(5) as i64),
                };
                return Ok((op, Some((held, reference))));
            }
            75..85 => match some_hold {
                None => Op::Tick,
                Some(hold) => Op::Capture {
                    hold,
                    destination: if self.rng.below(2) == 0 {
                        self.bank()?
                    } else {
                        self.wallet(other)?
                    },
                    amount: 1 + self.rng.below(2_500) as i64,
                    now,
                },
            },
            85..90 => some_hold.map_or(Op::Tick, |hold| Op::Release { hold }),
            90..95 => some_hold.map_or(Op::Tick, |hold| Op::Expire { hold, now }),
            _ => Op::Tick,
        };
        if let Op::Tick = op {
            self.clock += 1;
        }
        Ok((op, None))
    }

    /// Opens a hold's held account in the reference ledger.
    pub fn open_held(&mut self, held: AccountId, reference: &str) -> Result<(), Error> {
        self.oracle.open_account(held, &self.chart, reference)?;
        Ok(())
    }

    /// The draft and authorization for a posting operation.
    pub fn posting(&self, op: &Op) -> Result<(EntryDraft, Option<Authorization>), Error> {
        Ok(match *op {
            Op::Deposit { wallet, amount } => (
                EntryDraft::default()
                    .debit(self.bank()?, etb(amount))
                    .credit(self.wallet(wallet)?, etb(amount)),
                None,
            ),
            Op::Transfer { from, to, amount } => (
                EntryDraft::default()
                    .debit(self.wallet(from)?, etb(amount))
                    .credit(self.wallet(to)?, etb(amount)),
                None,
            ),
            Op::Adjust {
                from,
                to,
                amount,
                self_approved,
            } => (
                EntryDraft::default()
                    .debit(self.wallet(from)?, etb(amount))
                    .credit(self.wallet(to)?, etb(amount)),
                Some(Authorization {
                    reason: ReasonCode::new("compensation")?,
                    requested_by: actor("ops:1")?,
                    approved_by: Some(actor(if self_approved { "ops:1" } else { "ops:2" })?),
                }),
            ),
            _ => return Err("not a posting".into()),
        })
    }

    /// A reversal's authorization.
    pub fn booking_error() -> Result<Authorization, Error> {
        Ok(Authorization {
            reason: ReasonCode::new("booking_error")?,
            requested_by: actor("ops:1")?,
            approved_by: None,
        })
    }

    /// Applies `op` to the reference ledger, writing entry `entry`.
    pub fn apply(&mut self, op: &Op, entry: EntryId) -> Result<Outcome, Error> {
        let posted = |r: Result<&PostedEntry, LedgerError>| match r {
            Ok(e) => Outcome::Entry(Shape::of(e)),
            Err(e) => Outcome::Refused(e),
        };
        let held = |r: Result<&Hold, LedgerError>| match r {
            Ok(h) => Outcome::Hold(h.clone()),
            Err(e) => Outcome::Refused(e),
        };
        Ok(match *op {
            Op::Deposit { .. } | Op::Transfer { .. } => {
                let (draft, _) = self.posting(op)?;
                posted(self.oracle.post(entry, draft))
            }
            Op::Adjust { .. } => {
                let (draft, authorization) = self.posting(op)?;
                let authorization = authorization.ok_or("an adjustment needs approval")?;
                posted(self.oracle.post_adjustment(entry, draft, authorization))
            }
            Op::Reverse { target } => {
                posted(self.oracle.reverse(target, entry, Lane::booking_error()?))
            }
            Op::Place {
                hold,
                wallet,
                held: account,
                amount,
                expires_at,
            } => {
                let wallet = self.wallet(wallet)?;
                held(self.oracle.place_hold(
                    hold,
                    entry,
                    wallet,
                    account,
                    positive(amount)?,
                    expires_at,
                ))
            }
            Op::Capture {
                hold,
                destination,
                amount,
                now,
            } => held(
                self.oracle
                    .capture_hold(hold, entry, destination, positive(amount)?, now),
            ),
            Op::Release { hold } => held(self.oracle.release_hold(hold, entry)),
            Op::Expire { hold, now } => held(self.oracle.expire_hold(hold, entry, now)),
            Op::Tick => Outcome::Nothing,
        })
    }

    /// The reference ledger's own invariant checker (INV-1 to INV-25).
    pub fn check(&self) -> Result<(), Error> {
        check_invariants(&self.oracle).map_err(|v| format!("lane {}: {v:?}", self.number).into())
    }

    /// Holds still active in the reference ledger.
    pub fn active_holds(&self) -> usize {
        self.oracle
            .holds()
            .filter(|h| h.status() == HoldStatus::Active)
            .count()
    }
}
