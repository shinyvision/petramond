//! World replication: the server-side payload builders and the client-side
//! replica install path.
//!
//! The server serializes nothing here — payloads carry `Arc` handles to the
//! live section buffers ([`SectionBytes`]), so the in-process connection ships
//! refcount bumps and the TCP transport does the encoding on its own threads.
//! Per-cell block STATE rides in [`SectionStatesPayload`] using the save
//! codec's exact per-entry encodings (`DoorState::encode`, `Facing::to_u8`, …)
//! so replication is as lossless as a save/load roundtrip.
//!
//! The replica ([`ReplicaWorld`](super::ReplicaWorld)) has no generation, tick,
//! or save to run — those operations exist only on `ServerWorld`. Installs enter at the same post-ingest seam `poll()` uses for a
//! landed section (block-entity index, particle-emitter index, deep
//! classification, light + mesh queueing) but touch NO gen bookkeeping, save
//! bookkeeping, or `sim_guard` sets — on a replica those sets stay empty, so
//! the streaming-finality guard is structurally idle. For ABSENT sections the
//! replica answers physics/placement queries from the `ColumnPayload`
//! summaries (`WorldData::column_summaries`), mirroring how `column_gen`
//! answers on the server.
//!
//! Deliberately absent from section payloads (they replicate
//! elsewhere): container slot contents, furnace machine counters (only the
//! lit face ships — the replica installs a minimal lit stand-in so the mesher
//! renders it), mobs, and dropped items.

mod changes;
mod detached;
mod ingest;
mod payload;
mod provenance;
mod restate;
mod send_plan;

pub use changes::{column_key, section_key, Changes, FrameChanges};
pub use detached::{DetachedFold, TerrainEdit};
pub(crate) use payload::{detached_column_payload, detached_section_payload};
pub(in crate::world) use provenance::Provenance;
pub use provenance::{Cached, PieceRange, Resident, SectionContent};
pub use restate::decode_section_payload;

#[cfg(test)]
mod tests;
