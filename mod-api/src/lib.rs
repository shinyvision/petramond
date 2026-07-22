//! The petramond mod ABI: shared types crossing the engine↔WASM boundary.
//!
//! Both sides speak postcard-serialized enums over two raw entry points
//! (`host_dispatch` on the host, `mod_dispatch` on the guest); everything in this
//! crate is the vocabulary of those calls. The engine depends on this crate
//! directly; mods reach it through `mod-sdk`, which re-exports it and hides the
//! raw ABI (`mod_alloc`/`mod_free`/pointer packing) behind safe wrappers.
//!
//! # Evolving the ABI
//!
//! postcard has no schema: enum variants encode as their **declaration index**
//! and struct fields **positionally**, so reordering variants, reshaping a
//! variant's fields, or inserting a variant anywhere but the end all change the
//! wire encoding. Nothing is released and the only mods are the ones bundled in
//! this repo, so shape these types however stays cleanest and rebuild the mods
//! (`make mods`) — there is no external mod whose compiled copy must keep
//! decoding an old dialect. The host still disables (never crashes on) a mod
//! that sends a variant it cannot decode.

pub mod biome;
mod client;
mod data;
mod events;
mod ids;
mod protocol;
mod sched;
mod shape;
mod wire;

#[cfg(test)]
mod wire_pin;

pub use client::*;
pub use data::*;
pub use events::*;
pub use ids::*;
pub use protocol::*;
pub use sched::*;
pub use shape::*;
/// Bulk byte payloads ride the wire as postcard bytes either way; this
/// wrapper makes their (de)serialization a bulk copy instead of per-byte
/// serde visits. Re-exported so the SDK and host name one type.
pub use serde_bytes::ByteBuf;
pub use wire::*;
