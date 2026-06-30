//! Region files: each `r.<rx>.<rz>.dat` packs the modified sections of a 32×32
//! block of columns (a full vertical stack per column). Only sections the player
//! has modified are stored; the rest regenerate from the seed.
//!
//! A region is rewritten whole on every flush (new records merged over the
//! existing ones), which keeps the format trivial — header table of present
//! sections + their compressed records, no slot map / free list / compaction.
//!
//! Each section's slot is a `u16` local index packing its position within the
//! region: `lx` (5 bits) | `lz` (5 bits) | `cy − SECTION_MIN_CY` (5 bits). The
//! vertical range is `[SECTION_MIN_CY, SECTION_MAX_CY]` (20 sections), so the
//! biased `cy` fits in 5 bits and the whole index in a `u16`.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::chunk::{SectionPos, SECTION_MIN_CY};
use crate::save::codec::{put_u16, put_u32, Reader};

/// Columns per region edge (32 → 1024 columns per region, each a vertical stack).
pub const REGION_SHIFT: i32 = 5;
pub const REGION_SIZE: i32 = 1 << REGION_SHIFT;
/// Bit width of each packed field in the `u16` local index.
const FIELD_SHIFT: u16 = 5;

const MAGIC: u32 = 0x3252_434C; // "LCR2" little-endian (cubic section records)
const VERSION: u16 = 1;

pub fn region_of(pos: SectionPos) -> (i32, i32) {
    (pos.cx >> REGION_SHIFT, pos.cz >> REGION_SHIFT)
}

pub fn local_index(pos: SectionPos) -> u16 {
    let lx = (pos.cx & (REGION_SIZE - 1)) as u16;
    let lz = (pos.cz & (REGION_SIZE - 1)) as u16;
    let ly = (pos.cy - SECTION_MIN_CY) as u16;
    (ly << (2 * FIELD_SHIFT)) | (lz << FIELD_SHIFT) | lx
}

pub fn section_pos(rx: i32, rz: i32, lidx: u16) -> SectionPos {
    let mask = (REGION_SIZE - 1) as u16;
    let lx = (lidx & mask) as i32;
    let lz = ((lidx >> FIELD_SHIFT) & mask) as i32;
    let cy = (lidx >> (2 * FIELD_SHIFT)) as i32 + SECTION_MIN_CY;
    SectionPos::new(rx * REGION_SIZE + lx, cy, rz * REGION_SIZE + lz)
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
    put_u32(&mut buf, MAGIC);
    put_u16(&mut buf, VERSION);
    put_u16(&mut buf, records.len() as u16);
    // Stable order → reproducible files (nicer for diffing and tests).
    let mut entries: Vec<(&u16, &Vec<u8>)> = records.iter().collect();
    entries.sort_by_key(|(k, _)| **k);
    for (lidx, blob) in entries {
        put_u16(&mut buf, *lidx);
        put_u32(&mut buf, blob.len() as u32);
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
        // Sweep XZ (incl. negatives + region edges) across the full cy range so the
        // packed (lx | lz | cy) local index inverts back to the same section.
        for &(cx, cz) in &[(0, 0), (-1, -1), (31, 31), (-32, 32), (100, -77)] {
            for cy in crate::chunk::SECTION_MIN_CY..=crate::chunk::SECTION_MAX_CY {
                let pos = SectionPos::new(cx, cy, cz);
                let (rx, rz) = region_of(pos);
                let lidx = local_index(pos);
                assert_eq!(section_pos(rx, rz, lidx), pos, "({cx},{cy},{cz})");
            }
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
