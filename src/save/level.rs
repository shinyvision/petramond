//! `level.dat`: the per-world header — format version, seed, the world's
//! game-tick counter, and the mod world KV map. Per-player state (position,
//! inventory, effects…) lives in `players/<name>.dat` (see [`super::player`]).

use std::collections::BTreeMap;

use crate::save::codec::{get_kv_map, put_kv_map, put_u32, put_u64, Reader};

/// The one supported `level.dat` version. Only the CURRENT version decodes —
/// no legacy ladders. Bump this and wipe dev worlds when the layout changes.
/// v7 drops every player field (moved to `players/<name>.dat`) and makes
/// `tick` real (it was written as 0 through v6).
const VERSION: u32 = 7;

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
}

pub fn encode(seed: u32, tick: u64, world_kv: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut b = Vec::new();
    put_u32(&mut b, VERSION);
    put_u32(&mut b, seed);
    put_u64(&mut b, tick);
    put_kv_map(&mut b, world_kv);
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
    Some(LevelData {
        seed,
        tick,
        world_kv,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_roundtrips() {
        let kv = BTreeMap::from([
            ("petramond:time".to_owned(), vec![0x10, 0x20]),
            ("zombies:invuln_until".to_owned(), Vec::new()),
        ]);

        let bytes = encode(0xDEAD_BEEF, 12_345, &kv);
        let got = decode(&bytes).expect("decodes");

        assert_eq!(got.seed, 0xDEAD_BEEF);
        assert_eq!(got.tick, 12_345, "the world tick survives the round-trip");
        assert_eq!(got.world_kv, kv, "the mod world KV survives the round-trip");
    }

    #[test]
    fn a_stale_version_is_rejected_not_half_decoded() {
        // Only the current version loads (project rule: no legacy decode
        // paths; bump + wipe dev worlds instead). A stale blob must return
        // None so the session starts fresh.
        let mut bytes = encode(7, 0, &BTreeMap::new());
        bytes[0..4].copy_from_slice(&(VERSION - 1).to_le_bytes());
        assert!(decode(&bytes).is_none(), "stale version rejected");
        bytes[0..4].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(decode(&bytes).is_none(), "future version rejected");
    }
}
