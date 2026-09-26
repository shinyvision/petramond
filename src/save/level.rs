//! `level.dat`: the per-world header — format version, seed, the world's
//! game-tick counter, the world KV map, and the populated-chunk set (which
//! chunk columns already spawned their one-time worldgen herd — see
//! `mob::populate`). Per-player state (position, inventory, effects…) lives in
//! `players/<key>.dat` (see [`super::player`]).

use std::collections::{BTreeMap, BTreeSet};

use crate::save::codec::{get_kv_map, put_kv_map, put_u32, put_u64, Reader};
use crate::save::format::{Format, RecordError};
use petramond_world::chunk::ChunkPos;

/// The `level.dat` version this build writes.
/// v8 adds the populated-chunk set (worldgen herd one-time stock). The oldest
/// version this build reads; a later layout change adds an upgrade step to
/// [`FORMAT`] (see `save::format`).
const VERSION: u32 = 8;

/// The `level.dat` format and its upgrade chain.
pub const FORMAT: Format = Format::new("level.dat", VERSION, &[]);

/// Decoded `level.dat` contents.
pub struct LevelData {
    pub seed: u32,
    /// The world's game-tick counter at save time, restored through
    /// [`crate::world::World::restore_tick`] so scheduled ticks and
    /// tick-anchored state (the `petramond:clock` day cycle) continue across
    /// sessions instead of restarting at 0.
    pub tick: u64,
    /// The world KV map (`mod_id:key` → bytes).
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

/// Decode only the header's seed — the World Settings screen shows it
/// without opening the save.
pub fn read_seed(bytes: &[u8]) -> Result<u32, RecordError> {
    let body = FORMAT.upgrade_u32_record(bytes)?;
    Reader::new(&body)
        .u32()
        .ok_or(RecordError::corrupt(FORMAT.name, "seed", 4))
}

/// Decode a `level.dat`, migrating an older version first. A newer,
/// retired or malformed one is an error — never a fresh world.
pub fn decode(bytes: &[u8]) -> Result<LevelData, RecordError> {
    let body = FORMAT.upgrade_u32_record(bytes)?;
    let mut r = Reader::new(&body);
    // Offsets count from the start of the file (the version header is 4 bytes).
    let corrupt = |what, r: &Reader| RecordError::corrupt(FORMAT.name, what, 4 + r.offset());
    let seed = r.u32().ok_or_else(|| corrupt("seed", &r))?;
    let tick = r.u64().ok_or_else(|| corrupt("tick", &r))?;
    let world_kv = get_kv_map(&mut r).ok_or_else(|| corrupt("world kv", &r))?;
    let populated_count = r.u32().ok_or_else(|| corrupt("populated count", &r))?;
    let mut populated_columns = BTreeSet::new();
    for _ in 0..populated_count {
        let (Some(cx), Some(cz)) = (r.u32(), r.u32()) else {
            return Err(corrupt("populated column", &r));
        };
        populated_columns.insert(ChunkPos::new(cx as i32, cz as i32));
    }
    if !r.is_at_end() {
        return Err(corrupt("trailing bytes", &r));
    }
    Ok(LevelData {
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
    fn other_versions_are_typed_errors_not_a_fresh_world() {
        // Nothing older than the upgrade chain reaches and nothing newer than
        // this build decodes — and neither reads as "no level.dat", which
        // would start the world over with a fresh seed.
        let mut bytes = encode(7, 0, &BTreeMap::new(), &BTreeSet::new());
        bytes[0..4].copy_from_slice(&(FORMAT.oldest() - 1).to_le_bytes());
        assert!(matches!(
            decode(&bytes),
            Err(RecordError::Retired { found, .. }) if found == FORMAT.oldest() - 1
        ));
        bytes[0..4].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(matches!(
            decode(&bytes),
            Err(RecordError::Newer { found, .. }) if found == VERSION + 1
        ));
        assert!(read_seed(&bytes).is_err(), "the seed peek shares the gate");
    }

    #[test]
    fn a_truncated_file_reports_where_it_broke() {
        let bytes = encode(7, 99, &BTreeMap::new(), &BTreeSet::new());
        assert_eq!(
            decode(&bytes[..10]).err(),
            Some(RecordError::corrupt(FORMAT.name, "tick", 8)),
            "the seed ends at byte 8, where the tick is missing"
        );
        let mut long = bytes.clone();
        long.push(0);
        assert!(matches!(
            decode(&long),
            Err(RecordError::Corrupt {
                what: "trailing bytes",
                ..
            })
        ));
    }

    /// Golden `level.dat` v8, laid out by hand rather than by the encoder:
    /// seed 0x01020304, tick 5000, one world KV entry, two populated columns.
    /// A future layout keeps this decoding through its upgrade step.
    #[test]
    fn golden_level_v8_decodes() {
        let got = decode(include_bytes!("fixtures/level_v8.bin")).expect("v8 decodes");
        assert_eq!(got.seed, 0x0102_0304);
        assert_eq!(got.tick, 5000);
        assert_eq!(
            got.world_kv,
            BTreeMap::from([("test:k".to_owned(), vec![1, 2, 3])])
        );
        assert_eq!(
            got.populated_columns,
            BTreeSet::from([ChunkPos::new(-1, 2), ChunkPos::new(3, -4)])
        );
    }
}
