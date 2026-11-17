//! IDs for one lane and epoch of the offline workload.
//!
//! An ID packs the lane, the epoch, a kind and a number into the last 48
//! bits of a UUIDv7, so every ID says where it came from, and no two lanes
//! or epochs share one in the same database.

/// What an ID names.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Account = 1,
    Entry = 2,
    Device = 3,
    Voucher = 4,
    Client = 5,
}

/// The largest number of each kind in one epoch of one lane.
pub const MAX_N: u64 = (1 << 24) - 1;

/// The ID minter of one lane and epoch.
#[derive(Debug, Clone, Copy)]
pub struct Ids {
    pub lane: u64,
    pub epoch: u64,
}

impl Ids {
    /// Lane 0 to 255, epoch 0 to 4,095.
    pub fn new(lane: u64, epoch: u64) -> Result<Ids, String> {
        if lane > 0xff || epoch > 0xfff {
            return Err(format!("lane {lane} or epoch {epoch} is too large"));
        }
        Ok(Ids { lane, epoch })
    }

    pub fn text(&self, kind: Kind, n: u64) -> String {
        let packed = (self.lane << 40) | (self.epoch << 28) | ((kind as u64) << 24) | (n & MAX_N);
        format!("00000000-0000-7000-8000-{packed:012x}")
    }

    pub fn get<T: std::str::FromStr>(&self, kind: Kind, n: u64) -> Result<T, String>
    where
        T::Err: std::fmt::Display,
    {
        if n > MAX_N {
            return Err(format!("too many IDs of kind {kind:?} in one epoch"));
        }
        self.text(kind, n)
            .parse()
            .map_err(|e: T::Err| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_differ_by_lane_epoch_kind_and_number() -> Result<(), String> {
        let a = Ids::new(1, 2)?;
        let b = Ids::new(1, 3)?;
        let c = Ids::new(2, 2)?;
        let all = [
            a.text(Kind::Account, 5),
            a.text(Kind::Entry, 5),
            a.text(Kind::Account, 6),
            b.text(Kind::Account, 5),
            c.text(Kind::Account, 5),
        ];
        let unique: std::collections::BTreeSet<_> = all.iter().collect();
        assert_eq!(unique.len(), all.len());
        let id: mezgeb_core::AccountId = a.get(Kind::Account, 5)?;
        assert_eq!(id.to_string(), all[0]);
        assert!(Ids::new(256, 0).is_err());
        Ok(())
    }
}
