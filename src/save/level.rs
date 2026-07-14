//! `level.dat`: the per-world header — format version, seed, the world's
//! game-tick counter, the mod world KV map, and the populated-chunk set (which
//! chunk columns already spawned their one-time worldgen herd — see
//! `mob::populate`). Per-player state (position, inventory, effects…) lives in
//! `players/<name>.dat` (see [`super::player`]).

use std::collections::{BTreeMap, BTreeSet};

use crate::chunk::ChunkPos;
use crate::save::codec::{get_kv_map, put_kv_map, put_u32, put_u64, Reader};

/// The one supported `level.dat` version. Only the CURRENT version decodes —
/// no legacy ladders. Bump this and wipe dev worlds when the layout changes.
/// v8 adds the populated-chunk set (worldgen herd one-time stock).
const VERSION: u32 = 8;

/// Decoded `level.dat` contents.
pub struct LevelData {
    pub seed: u32,
    /// The world's game-tick counter at save time, restored through
    /// [`crate::world::World::restore_tick`] so scheduled ticks and
    /// tick-anchored state (the `petramond:clock` day cycle) continue across
    /// sessions instead of restarting at 0.
    pub tick: u64,
    /// The mod world KV map (`mod_id:key` → bytes; Phase 3b).
    pub world_kv: BTreeMap<String, Vec<u8>>,
    /// Chunk columns whose one-time worldgen herd already spawned. Restored
    /// through [`crate::world::World::set_populated_columns`] so the stock
    /// never re-mints across sessions.
    pub populated_columns: BTreeSet<ChunkPos>,
}

pub fn encode(
    seed: u32,
    tick: u64,
    world_kv: &BTreeMap<String, Vec<u8>>,
    populated_columns: &BTreeSet<ChunkPos>,
) -> Vec<u8> {
    let mut b = Vec::new();
    put_u32(&mut b, VERSION);
    put_u32(&mut b, seed);
    put_u64(&mut b, tick);
    put_kv_map(&mut b, world_kv);
    put_u32(&mut b, populated_columns.len() as u32);
    for chunk in populated_columns {
        put_u32(&mut b, chunk.cx as u32);
        put_u32(&mut b, chunk.cz as u32);
    }
    b
}

/// Decode a CURRENT-version `level.dat`. Any other version returns `None` —
/// the world starts fresh (pre-release, breaking saves is free).
pub fn decode(bytes: &[u8]) -> Option<LevelData> {
    let mut r = Reader::new(bytes);
    if r.u32()? != VERSION {
        return None;
    }
    let seed = r.u32()?;
    let tick = r.u64()?;
    let world_kv = get_kv_map(&mut r)?;
    let populated_count = r.u32()?;
    let mut populated_columns = BTreeSet::new();
    for _ in 0..populated_count {
        let cx = r.u32()? as i32;
        let cz = r.u32()? as i32;
        populated_columns.insert(ChunkPos::new(cx, cz));
    }
    Some(LevelData {
        seed,
        tick,
        world_kv,
        populated_columns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_roundtrips() {
        let kv = BTreeMap::from([
            ("petramond:time".to_owned(), vec![0x10, 0x20]),
            ("example:opaque".to_owned(), Vec::new()),
        ]);
        // Negative coords on purpose: the chunk set must round-trip sign-exact.
        let populated = BTreeSet::from([ChunkPos::new(-3, 17), ChunkPos::new(120, -9)]);

        let bytes = encode(0xDEAD_BEEF, 12_345, &kv, &populated);
        let got = decode(&bytes).expect("decodes");

        assert_eq!(got.seed, 0xDEAD_BEEF);
        assert_eq!(got.tick, 12_345, "the world tick survives the round-trip");
        assert_eq!(got.world_kv, kv, "the mod world KV survives the round-trip");
        assert_eq!(
            got.populated_columns, populated,
            "the populated-chunk set survives the round-trip"
        );
    }

    #[test]
    fn a_stale_version_is_rejected_not_half_decoded() {
        // Only the current version loads (project rule: no legacy decode
        // paths; bump + wipe dev worlds instead). A stale blob must return
        // None so the session starts fresh.
        let mut bytes = encode(7, 0, &BTreeMap::new(), &BTreeSet::new());
        bytes[0..4].copy_from_slice(&(VERSION - 1).to_le_bytes());
        assert!(decode(&bytes).is_none(), "stale version rejected");
        bytes[0..4].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(decode(&bytes).is_none(), "future version rejected");
    }
}
