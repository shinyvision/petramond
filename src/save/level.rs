use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

use crate::save::codec::{get_kv_map, put_kv_map, put_u32, put_u64, Reader};
use crate::save::format::{Format, RecordError};
use petramond_world::chunk::ChunkPos;

const VERSION: u32 = 8;

pub const FORMAT: Format = Format::new("level.dat", VERSION, &[]);

pub const FILE: &str = "level.dat";
pub const BACKUP: &str = "level.dat.bak";

pub struct LevelData {
    pub seed: u32,
    pub tick: u64,
    pub world_kv: BTreeMap<String, Vec<u8>>,
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

pub fn read_seed(bytes: &[u8]) -> Result<u32, RecordError> {
    let body = FORMAT.upgrade_u32_record(bytes)?;
    Reader::new(&body)
        .u32()
        .ok_or(RecordError::corrupt(FORMAT.name, "seed", 4))
}

pub fn decode(bytes: &[u8]) -> Result<LevelData, RecordError> {
    let body = FORMAT.upgrade_u32_record(bytes)?;
    let mut r = Reader::new(&body);
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

pub(super) fn backup_bytes(dir: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(dir.join(FILE)).ok()?;
    decode(&bytes).is_ok().then_some(bytes)
}

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
