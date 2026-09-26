//! `level.dat`: the per-world header — format version, seed, the world's
//! game-tick counter, the world KV map, and the populated-chunk set (which
//! chunk columns already spawned their one-time worldgen herd — see
//! `mob::populate`). Per-player state (position, inventory, effects…) lives in
//! `players/<key>.dat` (see [`super::player`]).
//!
//! Every save of the header keeps the previous readable one as
//! `level.dat.bak` in the same batch, and opening falls back to it when
//! `level.dat` is missing or corrupt. A header that is only from another
//! build (newer, or older than the migrations reach) never falls back: its
//! data is intact, and the world is refused instead.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

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

/// The header file, relative to the world directory.
pub const FILE: &str = "level.dat";
/// The previous readable header, kept beside it.
pub const BACKUP: &str = "level.dat.bak";

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

/// The bytes to keep as [`BACKUP`] before `dir`'s header is replaced: the
/// current `level.dat` when it decodes, else `None` (a corrupt header must
/// never overwrite a good backup). Read by the save writer as it builds the
/// batch that replaces the header.
pub(super) fn backup_bytes(dir: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(dir.join(FILE)).ok()?;
    decode(&bytes).is_ok().then_some(bytes)
}

/// Read `dir`'s header at open: `Ok(None)` for a world that has none yet.
/// A missing or corrupt `level.dat` falls back to [`BACKUP`] (the corrupt
/// bytes are quarantined first); a header from another build, or a corrupt
/// header with no readable backup, refuses the open — a world treated as
/// new would get a fresh seed and world KV written over the real ones.
pub(super) fn load(dir: &Path) -> io::Result<Option<LevelData>> {
    let primary = match std::fs::read(dir.join(FILE)) {
        Ok(bytes) => match decode(&bytes) {
            Ok(level) => return Ok(Some(level)),
            Err(error @ RecordError::Corrupt { .. }) => {
                if let Err(e) = super::format::quarantine(dir, Path::new(FILE), &bytes) {
                    log::error!("could not quarantine {}: {e}", dir.join(FILE).display());
                }
                Some(error)
            }
            Err(error) => return Err(error.into()),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let backup = match std::fs::read(dir.join(BACKUP)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return match primary {
                Some(error) => Err(error.into()),
                None => Ok(None),
            };
        }
        Err(e) => return Err(e),
    };
    let level = decode(&backup).map_err(|backup_error| match &primary {
        Some(error) => io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{error}; the backup is unreadable too ({backup_error})"),
        ),
        None => backup_error.into(),
    })?;
    match primary {
        Some(error) => log::warn!(
            "{} is unreadable ({error}); opening from {BACKUP} instead",
            dir.join(FILE).display()
        ),
        None => log::warn!(
            "{} is missing; opening from {BACKUP} instead",
            dir.join(FILE).display()
        ),
    }
    Ok(Some(level))
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

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("petramond-level-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_corrupt_header_falls_back_to_the_backup_and_is_quarantined() {
        let dir = temp_dir("fallback");
        let good = encode(41, 7, &BTreeMap::new(), &BTreeSet::new());
        std::fs::write(dir.join(BACKUP), &good).unwrap();
        std::fs::write(dir.join(FILE), &good[..6]).unwrap();
        let level = load(&dir).expect("opens").expect("a returning world");
        assert_eq!((level.seed, level.tick), (41, 7), "the backup's header");
        assert_eq!(
            std::fs::read(dir.join("quarantine").join(FILE)).unwrap(),
            &good[..6],
            "the corrupt bytes are kept"
        );

        std::fs::remove_file(dir.join(FILE)).unwrap();
        assert_eq!(
            load(&dir).expect("opens").map(|l| l.seed),
            Some(41),
            "a lost header falls back too"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_header_that_cannot_be_recovered_refuses_the_open() {
        let dir = temp_dir("refuse");
        assert!(load(&dir).expect("opens").is_none(), "a new world");

        let good = encode(41, 7, &BTreeMap::new(), &BTreeSet::new());
        std::fs::write(dir.join(FILE), &good[..6]).unwrap();
        assert_eq!(
            load(&dir).err().map(|e| e.kind()),
            Some(io::ErrorKind::InvalidData),
            "corrupt with no backup"
        );
        std::fs::write(dir.join(BACKUP), b"junk").unwrap();
        assert!(load(&dir).is_err(), "corrupt with a corrupt backup");

        // A newer build's header is intact: never replaced by the backup.
        let mut newer = good.clone();
        newer[0..4].copy_from_slice(&(VERSION + 1).to_le_bytes());
        std::fs::write(dir.join(FILE), &newer).unwrap();
        std::fs::write(dir.join(BACKUP), &good).unwrap();
        assert!(load(&dir).is_err(), "a newer header refuses the open");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_readable_header_becomes_the_backup() {
        let dir = temp_dir("backup-bytes");
        assert_eq!(backup_bytes(&dir), None, "nothing to keep yet");
        let good = encode(3, 9, &BTreeMap::new(), &BTreeSet::new());
        std::fs::write(dir.join(FILE), &good).unwrap();
        assert_eq!(backup_bytes(&dir), Some(good.clone()));
        std::fs::write(dir.join(FILE), &good[..5]).unwrap();
        assert_eq!(backup_bytes(&dir), None, "a corrupt header is not kept");
        let _ = std::fs::remove_dir_all(&dir);
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
