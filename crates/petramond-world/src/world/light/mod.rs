pub mod bake;
pub mod batch;
pub mod flood;
pub mod incremental;
pub mod neighborhood;

pub mod shape;
pub mod skylight;

#[cfg(test)]
mod parity_tests;
#[cfg(test)]
mod test_fixture;

use crate::chunk::SECTION_SIZE;

pub use bake::{bake_section, LightBakeOutput, SectionBakeJob};
pub use batch::{group_positions, run_light_bake_batch, snapshot_batch, LightBatchJob};
pub use incremental::{edit_relightable, relight_edits, RelitSection};

pub use skylight::cover_change_affects_section;

#[inline]
pub fn region_bit(dx: i32, dy: i32, dz: i32) -> u32 {
    1 << (((dy + 1) * 9 + (dz + 1) * 3 + (dx + 1)) as u32)
}

pub const REGION_ALL: u32 = (1 << 27) - 1;

pub static ZERO_CUBE: [crate::light::LightRgb; crate::chunk::SECTION_VOLUME] =
    [crate::light::LightRgb::ZERO; crate::chunk::SECTION_VOLUME];

/// Which mesh-sampling regions differ between an old and a freshly baked light
/// cube: the bit for delta `d` is set when a changed cell lies on the border
/// plane/edge/corner the neighbour at `d` samples through its one-cell mesh
/// pad (the centre bit: any change at all). `old = None` reads as the uniform
/// `fallback` the live accessors use for an absent cube.
pub fn cube_region_changes<T: Copy + PartialEq>(old: Option<&[T]>, new: &[T], fallback: T) -> u32 {
    #[inline]
    fn axis_bits(local: usize) -> u32 {
        if local == 0 {
            0b011
        } else if local == SECTION_SIZE - 1 {
            0b110
        } else {
            0b010
        }
    }
    fn cell_bits(i: usize) -> u32 {
        let (lx, ly, lz) = crate::chunk::section_local(i);
        let (xb, yb, zb) = (axis_bits(lx), axis_bits(ly), axis_bits(lz));
        let mut bits = 0u32;
        for dy in 0..3u32 {
            if yb & (1 << dy) == 0 {
                continue;
            }
            for dz in 0..3u32 {
                if zb & (1 << dz) == 0 {
                    continue;
                }
                for dx in 0..3u32 {
                    if xb & (1 << dx) != 0 {
                        bits |= 1 << (dy * 9 + dz * 3 + dx);
                    }
                }
            }
        }
        bits
    }
    let mut mask = 0u32;
    match old {
        Some(old) => {
            debug_assert_eq!(old.len(), new.len());
            let (old8, new8) = (old.chunks_exact(8), new.chunks_exact(8));
            for (w, (o, n)) in old8.zip(new8).enumerate() {
                if o == n {
                    continue;
                }
                for b in 0..8 {
                    if o[b] != n[b] {
                        mask |= cell_bits(w * 8 + b);
                    }
                }
                if mask == REGION_ALL {
                    return mask;
                }
            }
        }
        None => {
            for (i, &n) in new.iter().enumerate() {
                if n != fallback {
                    mask |= cell_bits(i);
                    if mask == REGION_ALL {
                        return mask;
                    }
                }
            }
        }
    }
    mask
}

pub const NBHD: usize = 3 * SECTION_SIZE;
pub const NBHD_VOLUME: usize = NBHD * NBHD * NBHD;
pub const NBHD_AREA: usize = NBHD * NBHD;

#[inline]
pub fn nbhd_idx(x: usize, y: usize, z: usize) -> usize {
    neighborhood::cube_idx(NBHD, x, y, z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{section_idx, SECTION_VOLUME, SKY_FULL};

    #[test]
    fn region_change_masks_map_changed_cells_to_their_sampling_neighbours() {
        let old = vec![0u8; SECTION_VOLUME];

        let mut new = old.clone();
        new[section_idx(8, 8, 8)] = 4;
        assert_eq!(
            cube_region_changes(Some(&old), &new, 0),
            region_bit(0, 0, 0)
        );

        let mut new = old.clone();
        new[section_idx(0, 8, 8)] = 4;
        assert_eq!(
            cube_region_changes(Some(&old), &new, 0),
            region_bit(0, 0, 0) | region_bit(-1, 0, 0)
        );

        let mut new = old.clone();
        new[section_idx(15, 15, 15)] = 4;
        let mask = cube_region_changes(Some(&old), &new, 0);
        assert_eq!(mask.count_ones(), 8);
        assert_ne!(mask & region_bit(1, 1, 1), 0);
        assert_ne!(mask & region_bit(1, 0, 0), 0);
        assert_eq!(mask & region_bit(-1, 0, 0), 0);

        assert_eq!(cube_region_changes(None, &old, 0), 0);
        let full = vec![SKY_FULL; SECTION_VOLUME];
        assert_eq!(cube_region_changes(None, &full, SKY_FULL), 0);
        assert_ne!(cube_region_changes(None, &full, 0), 0);
    }
}
