//! The four wire enums — [`HostCall`]/[`HostRet`] (guest→host request/reply)
//! and [`GuestCall`]/[`GuestRet`] (host→guest) — plus the worldgen write
//! alias. Evolution rules live in the crate docs; the recorded encoding
//! lives in `wire_pin`.

mod guest;
mod host;

#[cfg(test)]
mod tests;

pub use guest::{FeaturePlacement, GenOutput, GenWrite, GuestRet, HostCall, StructurePlacement};
pub use host::{GuestCall, HostRet, MemoClaim};
