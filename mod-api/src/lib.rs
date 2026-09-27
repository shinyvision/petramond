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
//! and struct fields **positionally**, so a compiled guest only understands the
//! exact dialect it was built against. Every guest therefore declares the
//! [`AbiVersion`] it speaks, and the host refuses a different major at load
//! with a readable [`AbiRejection`] (see [`negotiate`] for the handshake and
//! capability negotiation). The rules for changing this crate:
//!
//! - **Minor bump** (`ABI_VERSION.minor`): the ABI only GROWS — a variant
//!   appended at the END of a call/reply enum (for the nested [`HostCall`]:
//!   a call at the end of its domain enum, or a domain at the end of
//!   [`HostCall`]), or a new [`Capabilities`] bit.
//!   Older guests keep working: a call they do not know is answered with
//!   [`GuestRet::Unsupported`], and a newer guest's unknown call to an older
//!   host gets [`HostRet::Unsupported`]. A new reply variant may only answer a
//!   new call, and a new event payload may only reach a handler registered for
//!   its (new) kind, so an older peer never has to decode one.
//! - **Major bump**: anything that changes an existing encoding — reordering
//!   or removing variants, adding/removing/reshaping fields. Every compiled
//!   guest of the old major is refused at load; rebuild the bundled mods
//!   (`make mods`).
//!
//! `wire_pin` records the encoding of every variant so neither kind of change
//! happens by accident.
//!
//! # Legality and errors
//!
//! Every host call is declared with its [`Legality`] (the instance sides it
//! reaches, whether it is confined to `mod_init`, whether it mutates); the
//! host derives its gates from [`HostCall::legality`], never from a list of
//! its own. A refused call answers a typed [`HostError`] (see [`ErrorCode`]
//! for which refusals a mod can recover from).

mod abi;
pub mod biome;
pub mod capture;
mod client;
mod data;
mod error;
mod events;
mod files;
mod ids;
pub mod json;
mod legality;
mod limits;
mod media;
mod protocol;
mod sched;
mod shape;
mod wire;
mod worldgen;

#[cfg(test)]
mod wire_pin;

pub use abi::*;
pub use capture::{
    ClientCapturedClock, ClientCapturedEnv, ClientCapturedMod, ClientCapturedSession,
    ClientCapturedView, ClientColumnPresence, ClientEnvelope, ClientEnvelopeEntry,
    ClientEventsPhase, ClientEventsReport, ClientFrameEnvelope, ClientPieceInfo, ClientPieceKind,
    ClientPopulation, ClientPose, ClientPresence, ClientPresentationStateData, ClientRosterEntry,
    ClientStateEnvelope, ClientStateKey, ClientStateSelect, ClientStateTicketData,
};
pub use client::*;
pub use data::*;
pub use error::*;
pub use events::*;
pub use files::*;
pub use ids::*;
pub use legality::*;
pub use limits::*;
pub use media::*;
pub use protocol::*;
pub use sched::*;
pub use serde_bytes::ByteBuf;
pub use shape::*;
pub use wire::*;
pub use worldgen::*;
