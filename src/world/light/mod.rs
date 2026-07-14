//! Async section light baking plus the light-shape rules used by the floods.
//!
//! Keep this subsystem split by responsibility: the queue owns jobs and workers,
//! `neighborhood` owns snapshot assembly, `skylight` owns sky-cover planning,
//! `flood` owns propagation, and `shape` owns per-block boundary rules.

mod flood;
mod neighborhood;
mod queue;
mod shape;
mod skylight;

use crate::chunk::SECTION_SIZE;

pub(super) use queue::{run_light_bake, LightBakeJob, LightBakeQueue, LightBakeResult};
pub(super) use skylight::cover_change_affects_section;

/// Bit for neighbour delta `(dx, dy, dz)` (each −1/0/+1) in a
/// [`cube_region_changes`] mask; the centre bit `(0,0,0)` reads "any change".
#[inline]
pub(in crate::world) fn region_bit(dx: i32, dy: i32, dz: i32) -> u32 {
    1 << (((dy + 1) * 9 + (dz + 1) * 3 + (dx + 1)) as u32)
}

/// All 27 region bits set: every sampling neighbour saw a change.
pub(in crate::world) const REGION_ALL: u32 = (1 << 27) - 1;

/// An all-dark cube, for diffing a dropped (`None`, reads-as-zero) block-light
/// buffer against a previously cached one.
pub(in crate::world) static ZERO_CUBE: [u8; crate::chunk::SECTION_VOLUME] =
    [0; crate::chunk::SECTION_VOLUME];

/// Which mesh-sampling regions differ between an old and a freshly baked light
/// cube: the bit for delta `d` is set when a changed cell lies on the border
/// plane/edge/corner the neighbour at `d` samples through its one-cell mesh
/// pad (the centre bit: any change at all). `old = None` reads as the uniform
/// `fallback` the live accessors use for an absent cube.
pub(in crate::world) fn cube_region_changes(old: Option<&[u8]>, new: &[u8], fallback: u8) -> u32 {
    #[inline]
    fn axis_bits(local: usize) -> u32 {
        // Bit 0: delta −1, bit 1: delta 0, bit 2: delta +1 along one axis.
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
            // Word-compare fast path: almost every rebake changes few cells.
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

/// Temporary perf-session diagnostics (see `tooling::stream::stage_stats`).
pub(crate) fn stage_stats() -> (
    &'static std::sync::atomic::AtomicU64,
    &'static std::sync::atomic::AtomicU64,
) {
    (&queue::LIGHT_STAGE_NS, &queue::LIGHT_STAGE_JOBS)
}

/// Side length of the light flood neighbourhood (3 sections).
pub(super) const NBHD: usize = 3 * SECTION_SIZE;
pub(super) const NBHD_VOLUME: usize = NBHD * NBHD * NBHD;
pub(super) const NBHD_AREA: usize = NBHD * NBHD;

#[inline]
pub(super) fn nbhd_idx(x: usize, y: usize, z: usize) -> usize {
    (y * NBHD + z) * NBHD + x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{section_idx, SECTION_VOLUME, SKY_FULL};

    #[test]
    fn region_change_masks_map_changed_cells_to_their_sampling_neighbours() {
        let old = vec![0u8; SECTION_VOLUME];

        // Interior change: only this section's own mesh samples it.
        let mut new = old.clone();
        new[section_idx(8, 8, 8)] = 4;
        assert_eq!(
            cube_region_changes(Some(&old), &new, 0),
            region_bit(0, 0, 0)
        );

        // Border-plane change: the facing neighbour's pad samples it too.
        let mut new = old.clone();
        new[section_idx(0, 8, 8)] = 4;
        assert_eq!(
            cube_region_changes(Some(&old), &new, 0),
            region_bit(0, 0, 0) | region_bit(-1, 0, 0)
        );

        // Corner cell: centre, three faces, three edges, and the corner.
        let mut new = old.clone();
        new[section_idx(15, 15, 15)] = 4;
        let mask = cube_region_changes(Some(&old), &new, 0);
        assert_eq!(mask.count_ones(), 8);
        assert_ne!(mask & region_bit(1, 1, 1), 0);
        assert_ne!(mask & region_bit(1, 0, 0), 0);
        assert_eq!(mask & region_bit(-1, 0, 0), 0);

        // An absent old cube reads as the uniform fallback the accessors use.
        assert_eq!(cube_region_changes(None, &old, 0), 0);
        let full = vec![SKY_FULL; SECTION_VOLUME];
        assert_eq!(cube_region_changes(None, &full, SKY_FULL), 0);
        assert_ne!(cube_region_changes(None, &full, 0), 0);
    }
}
