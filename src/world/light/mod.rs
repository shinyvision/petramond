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

pub(super) use queue::LightBakeQueue;
pub(super) use skylight::cover_change_affects_section;

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
