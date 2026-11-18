//! The JSON the API sends and receives (`openapi/mezgeb.v1.yaml`), and the
//! conversions at the boundary: amounts as decimal strings, times as
//! RFC 3339, IDs as UUIDs.
//!
//! Responses are always these explicit types, built field by field, never
//! engine types serialized as they are: nothing internal can leak out.

mod account;
mod adjustment;
mod audit;
mod entry;
mod hold;
mod money;
mod offline;
mod period;
mod recon;
mod timestamp;

pub use account::*;
pub use adjustment::*;
pub use audit::*;
pub use entry::*;
pub use hold::*;
pub use money::*;
pub use offline::*;
pub use period::*;
pub use recon::*;
pub use timestamp::*;
