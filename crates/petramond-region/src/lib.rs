//! Region files: each `r.<rx>.<rz>.dat` packs the modified sections of a 32×32
//! block of columns (a full vertical stack per column). Only sections the player
//! has modified are stored; the rest regenerate from the seed. The same
//! container is shared by `region/`, `explored/`, and `colgen/`.
//!
//! Format v3 (2026-09-26) is append-with-index, so a flush costs what it
//! writes, not what the region already holds:
//! - A fixed header carries two index SLOTS. A slot names where the current
//!   record index lives, its length and checksum, and a sequence number.
//! - A merge APPENDS the new record bodies, then a fresh index covering every
//!   live record, then overwrites the INACTIVE slot with the next sequence
//!   number. Nothing a reader may hold is ever overwritten in place (bodies
//!   and older indexes stay where they are), and a torn or unflushed slot
//!   fails its checksum, so the reader falls back to the previous slot and
//!   the previous index — the state before the merge.
//! - Superseded bodies and indexes are garbage. Once the garbage outweighs
//!   the live records (and a floor), the merge COMPACTS instead: the live
//!   records are rewritten into a fresh file that atomically replaces the
//!   old one. Compaction costs O(live) after at least O(live) bytes were
//!   appended, so writes stay proportional to what changed, amortized.
//! - Appends are not flushed here. A caller that needs them durable (the
//!   save journal) writes every region of a batch first and then calls
//!   [`sync`] once per file, so a save cycle pays one flush per touched file
//!   instead of one per rewrite.
//!
//! Format v2 (contiguous header table + packed bodies, rewritten whole on
//! every flush) is still read; the first merge into a v2 file rewrites it as
//! v3. v1 (interleaved) files are rejected.
//!
//! Each section's slot is a `u16` local index packing its position within the
//! region: `lx` (5 bits) | `lz` (5 bits) | `cy − SECTION_MIN_CY` (5 bits). The
//! vertical range is `[SECTION_MIN_CY, SECTION_MAX_CY]` (20 sections), so the
//! biased `cy` fits in 5 bits and the whole index in a `u16`.

mod container;

pub use container::{merge_region, read_region_indices, sync, MergePolicy, RegionReader};

use std::path::{Path, PathBuf};

use petramond_world::chunk::{SectionPos, SECTION_MIN_CY};

/// Columns per region edge (32 → 1024 columns per region, each a vertical stack).
pub const REGION_SHIFT: i32 = 5;
pub const REGION_SIZE: i32 = 1 << REGION_SHIFT;
/// Bit width of each packed field in the `u16` local index.
const FIELD_SHIFT: u16 = 5;

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

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::chunk::SECTION_MAX_CY;

    #[test]
    fn region_math_roundtrips() {
        // Sweep XZ (incl. negatives + region edges) across the full cy range so the
        // packed (lx | lz | cy) local index inverts back to the same section.
        for &(cx, cz) in &[(0, 0), (-1, -1), (31, 31), (-32, 32), (100, -77)] {
            for cy in SECTION_MIN_CY..=SECTION_MAX_CY {
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
