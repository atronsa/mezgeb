//! Deterministic simulator (spec §10).
//!
//! Run: `cargo run -p mezgeb-sim -- --seed 42 --steps 100000`
//! The same seed always produces the same run, so any failure is replayable.

mod rng;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed = arg(&args, "--seed").unwrap_or(1);
    let steps = arg(&args, "--steps").unwrap_or(10_000);
    let mut _rng = rng::SplitMix64::new(seed);

    // TODO (spec §10):
    // 1. Build an in-memory StateView.
    // 2. Each step, randomly: create accounts, transfers, pending/post/void,
    //    issue vouchers, generate offline receipts, deliver them reordered,
    //    duplicated, delayed, from payer or payee; clone a device.
    // 3. After every step, check INV-1..INV-12; on failure print seed + step.
    println!("mezgeb-sim: seed={seed} steps={steps} (no workload yet)");
}

fn arg(args: &[String], name: &str) -> Option<u64> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1)?.parse().ok()
}
