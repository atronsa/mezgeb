//! Ledger REST service (ADR-0009, spec §9).
//!
//! Handlers authenticate the caller, then call the posting templates
//! (`mezgeb-ops`) and the engine (`mezgeb-engine`). Built in steps 2.06–2.08.

#[tokio::main]
async fn main() {
    // TODO: config, connect PgStore, run migrations, serve the REST API.
    println!("mezgeb-api: not implemented yet (see docs/spec.md §9)");
}
