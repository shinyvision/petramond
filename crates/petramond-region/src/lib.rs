//! `r.<rx>.<rz>.dat` holds the modified sections of a 32x32 column block, a full vertical stack
//! per column. Untouched sections regen from the seed. `region/`, `explored/` and `colgen/` all
//! share this container.
//!
//! v3 is append-with-index, so a flush costs what it writes, not the whole region.
//! - The header has two index slots. Each names the live record index plus its length, checksum
//!   and sequence number.
//! - A merge appends new bodies, writes a fresh index over live records, and overwrites the
//!   inactive slot with the next sequence number. Nothing a reader holds is overwritten in place.
//!   A torn slot fails its checksum, so readers fall back to the other slot and index.
//! - Once dead bodies and indexes outweigh live records plus a floor, the merge compacts instead:
//!   live records get rewritten into a fresh file that's swapped in atomically.
//! - Appends aren't flushed here. The save journal writes every region of a batch, then calls
//!   [`sync`] once per file.
//!
//! v2 (whole file rewritten per flush) is still readable, and the first merge upgrades it to v3.
//! v1 is rejected.
//!
//! Section slot: `u16` = `lx` (5 bits) | `lz` (5 bits) | `cy - SECTION_MIN_CY` (5 bits). There are
//! 20 vertical sections, so the biased `cy` fits in 5 bits.

mod container;

pub use container::{merge_region, read_region_indices, sync, MergePolicy, RegionReader};

use std::path::{Path, PathBuf};

use petramond_world::chunk::{SectionPos, SECTION_MIN_CY};

pub const REGION_SHIFT: i32 = 5;
pub const REGION_SIZE: i32 = 1 << REGION_SHIFT;
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
