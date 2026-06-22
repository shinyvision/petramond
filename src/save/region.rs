//! Region files: each `r.<rx>.<rz>.dat` packs a 32×32 block of chunks. Only
//! chunks the player has modified are stored; the rest regenerate from the seed.
//!
//! A region is rewritten whole on every flush (new records merged over the
//! existing ones), which keeps the format trivial — header table of present
//! chunks + their compressed records, no slot map / free list / compaction.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::chunk::ChunkPos;
use crate::save::codec::{Reader, Writer};

/// Chunks per region edge (32 → 1024 chunks per region).
pub const REGION_SHIFT: i32 = 5;
pub const REGION_SIZE: i32 = 1 << REGION_SHIFT;

const MAGIC: u32 = 0x3152_434C; // "LCR1" little-endian
const VERSION: u16 = 1;

pub fn region_of(pos: ChunkPos) -> (i32, i32) {
    (pos.cx >> REGION_SHIFT, pos.cz >> REGION_SHIFT)
}

pub fn local_index(pos: ChunkPos) -> u16 {
    let lx = (pos.cx & (REGION_SIZE - 1)) as u16;
    let lz = (pos.cz & (REGION_SIZE - 1)) as u16;
    lz * REGION_SIZE as u16 + lx
}

pub fn chunk_pos(rx: i32, rz: i32, lidx: u16) -> ChunkPos {
    let lx = (lidx % REGION_SIZE as u16) as i32;
    let lz = (lidx / REGION_SIZE as u16) as i32;
    ChunkPos::new(rx * REGION_SIZE + lx, rz * REGION_SIZE + lz)
}

pub fn region_path(region_dir: &Path, rx: i32, rz: i32) -> PathBuf {
    region_dir.join(format!("r.{rx}.{rz}.dat"))
}

/// Parse `r.<rx>.<rz>.dat` back into region coords (handles negatives).
pub fn parse_region_name(path: &Path) -> Option<(i32, i32)> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix("r.")?.strip_suffix(".dat")?;
    let (a, b) = rest.split_once('.')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Read a region into `local_index -> compressed record`. Missing file = empty.
/// Corrupt file = `InvalidData` error (caller decides whether to ignore).
pub fn read_region(path: &Path) -> io::Result<HashMap<u16, Vec<u8>>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(e),
    };
    let mut r = Reader::new(&bytes);
    let parsed = (|| {
        if r.u32()? != MAGIC || r.u16()? != VERSION {
            return None;
        }
        let count = r.u16()? as usize;
        let mut out = HashMap::with_capacity(count);
        for _ in 0..count {
            let lidx = r.u16()?;
            let len = r.u32()? as usize;
            out.insert(lidx, r.bytes(len)?.to_vec());
        }
        Some(out)
    })();
    parsed.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "corrupt region file"))
}

/// The present local indices only (for building the load manifest cheaply).
pub fn read_region_indices(path: &Path) -> io::Result<Vec<u16>> {
    Ok(read_region(path)?.into_keys().collect())
}

/// Atomically write a whole region (tmp + rename). An empty map removes the file.
pub fn write_region(path: &Path, records: &HashMap<u16, Vec<u8>>) -> io::Result<()> {
    if records.is_empty() {
        return match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        };
    }
    let mut buf = Vec::new();
    buf.put_u32(MAGIC);
    buf.put_u16(VERSION);
    buf.put_u16(records.len() as u16);
    // Stable order → reproducible files (nicer for diffing and tests).
    let mut entries: Vec<(&u16, &Vec<u8>)> = records.iter().collect();
    entries.sort_by_key(|(k, _)| **k);
    for (lidx, blob) in entries {
        buf.put_u16(*lidx);
        buf.put_u32(blob.len() as u32);
        buf.extend_from_slice(blob);
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &buf)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_math_roundtrips() {
        for &(cx, cz) in &[(0, 0), (-1, -1), (31, 31), (-32, 32), (100, -77)] {
            let pos = ChunkPos::new(cx, cz);
            let (rx, rz) = region_of(pos);
            let lidx = local_index(pos);
            assert_eq!(chunk_pos(rx, rz, lidx), pos, "({cx},{cz})");
        }
    }

    #[test]
    fn name_parse_roundtrips() {
        let dir = Path::new("/tmp/region");
        for &(rx, rz) in &[(0, 0), (-3, 5), (12, -1)] {
            let p = region_path(dir, rx, rz);
            assert_eq!(parse_region_name(&p), Some((rx, rz)));
        }
    }
}
