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
//! The replica ([`WorldRole::ClientReplica`]) never generates, ticks, or
//! saves: installs enter at the same post-ingest seam `poll()` uses for a
//! landed section (block-entity index, particle-emitter index, deep
//! classification, light + mesh queueing) but touch NO gen bookkeeping, save
//! bookkeeping, or `sim_guard` sets — on a replica those sets stay empty, so
//! the streaming-finality guard is structurally idle. For ABSENT sections the
//! replica answers physics/placement queries from the `ColumnPayload`
//! summaries (`World::column_summaries`), mirroring how `column_gen` answers
//! for the combined world.
//!
//! Deliberately absent from section payloads (they replicate
//! elsewhere): container slot contents, furnace machine counters (only the
//! lit face ships — the replica installs a minimal lit stand-in so the mesher
//! renders it), mobs, and dropped items.

use std::collections::HashMap;

mod ingest;
mod payload;
mod send_plan;

#[cfg(test)]
mod tests;

/// Sparse map → sorted wire entries, so identical state encodes identically
/// (the same reproducibility rule as the save codec's `put_indexed`).
fn sorted_entries<T, U>(map: &HashMap<u16, T>, mut f: impl FnMut(&T) -> U) -> Vec<(u16, U)> {
    let mut out: Vec<(u16, U)> = map.iter().map(|(&cell, v)| (cell, f(v))).collect();
    out.sort_unstable_by_key(|(cell, _)| *cell);
    out
}

/// Wire entries → sparse map (the install-side inverse of [`sorted_entries`]).
fn map_entries<T, U: Copy>(entries: &[(u16, U)], mut f: impl FnMut(U) -> T) -> HashMap<u16, T> {
    entries.iter().map(|&(cell, v)| (cell, f(v))).collect()
}
