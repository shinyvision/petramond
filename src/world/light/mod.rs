//! Async section light baking plus the light-shape rules used by the floods.
//!
//! Keep this subsystem split by responsibility: the queue owns jobs and workers,
//! `neighborhood` owns snapshot assembly, `skylight` owns heightmap planning,
//! `flood` owns propagation, and `shape` owns per-block boundary rules.

mod flood;
mod neighborhood;
mod queue;
mod shape;
mod skylight;

use crate::chunk::SECTION_SIZE;

pub(super) use queue::LightBakeQueue;

/// Side length of the light flood neighbourhood (3 sections).
pub(super) const NBHD: usize = 3 * SECTION_SIZE;
pub(super) const NBHD_VOLUME: usize = NBHD * NBHD * NBHD;
pub(super) const NBHD_AREA: usize = NBHD * NBHD;

#[inline]
pub(super) fn nbhd_idx(x: usize, y: usize, z: usize) -> usize {
    (y * NBHD + z) * NBHD + x
}
