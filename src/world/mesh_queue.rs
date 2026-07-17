use rustc_hash::FxHashSet;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::chunk::SectionPos;

use super::store::LoadTarget;

mod cpu_release;
mod light_pump;
mod mesh_jobs;
mod prediction;
mod pump;
mod sealing;

#[cfg(test)]
mod tests;

/// Minimum useful mesh submissions per pump. With the game-side budget intentionally set
/// to 1, a literal one-section budget makes the cubic streamer visibly crawl; this keeps
/// the tiny budget useful without multiplying larger diagnostic/tooling budgets.
const MIN_MESH_JOBS_PER_PUMP: usize = 16;
/// Scan past sections that are stale, no-mesh, or waiting on light so the budget still
/// launches useful work whenever any nearby section is ready. During streaming most
/// popped candidates PARK (light in flight / hidden deep) rather than submit, so the
/// scan must run well ahead of the submit count or parking throttles discovery to a
/// frame-quantized trickle. The submit time budget bounds the scan's real cost.
const CANDIDATE_SCAN_PER_MESH_JOB: usize = 4;
/// Bound result drains by TIME, not count: installs are cheap (Arc swaps + map
/// inserts), so a fixed small count needlessly frame-quantized streaming bursts
/// (24/frame = seconds of trickle for a flight burst the pool finished long ago).
/// The floor guarantees progress regardless of clock behaviour.
const RESULT_DRAIN_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(700);
const RESULT_DRAIN_MIN: usize = 24;
/// Cap on mesh jobs in flight in the shared pool. The pool queue is priority-ordered
/// (nearest first), so a fresh edit no longer queues behind the streaming backlog the
/// way it did with the old FIFO channel — this cap only bounds snapshot memory held
/// by queued jobs and stale-priority momentum after a target move. The backlog beyond
/// it stays in `dirty_meshes`, re-sorted NEAREST-FIRST every frame.
///
/// Sized to keep the worker pool FED for a frame of bulk streaming (~4 jobs/worker at
/// ~1 ms/job, 60 fps). The old fixed 16 admission-limited streaming to ~16 meshes per
/// frame while workers idled: RD32 flight grew a ~10k dirty backlog and the meshed
/// frontier visibly lagged the player.
fn max_mesh_jobs_in_flight() -> usize {
    static CAP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CAP.get_or_init(|| (crate::worker::JobPool::default_threads() * 4).clamp(16, 256))
}
/// Soft main-thread budget for mesh-job snapshot submission. One useful submission is
/// always allowed; after that, the pump yields to rendering once it burns this much CPU.
const MESH_SUBMIT_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(2_000);
/// Mesh-pump frames a column must stay upload-quiet before its CPU mesh buffers are
/// released (~10 s at 60 fps). Releasing too early amplifies streaming work: any
/// repack of the column then has to remesh the released sections first.
pub(super) const MESH_RELEASE_DELAY_FRAMES: u64 = 600;
/// How often the release sweep scans `mesh_release_after` (it iterates the whole map,
/// so keep it off the every-frame path).
const MESH_RELEASE_SWEEP_INTERVAL: u64 = 64;

/// Set of sections awaiting a remesh. With `World`'s section map private, every
/// path that dirties a section pushes here and `remove_section` pulls it back out —
/// so the set alone says what needs meshing. Drained NEAREST-FIRST to the load
/// centre so the terrain around the player meshes before the edges.
pub(super) struct DirtyMeshQueue {
    pending: FxHashSet<SectionPos>,
    /// Entries cache their priority once. Removal is lazy: `pending` remains the
    /// source of truth and stale heap rows are skipped when popped.
    heap: BinaryHeap<Reverse<(i64, i32, i32, i32)>>,
    target: Option<LoadTarget>,
}

impl Default for DirtyMeshQueue {
    fn default() -> Self {
        Self {
            pending: FxHashSet::default(),
            heap: BinaryHeap::new(),
            target: None,
        }
    }
}

impl DirtyMeshQueue {
    fn entry(target: Option<LoadTarget>, pos: SectionPos) -> Reverse<(i64, i32, i32, i32)> {
        Reverse((
            target.map_or(0, |t| t.section_priority_key(pos)),
            pos.cx,
            pos.cy,
            pos.cz,
        ))
    }

    fn rebuild(&mut self, target: Option<LoadTarget>) {
        self.target = target;
        self.heap.clear();
        self.heap
            .extend(self.pending.iter().copied().map(|p| Self::entry(target, p)));
    }

    pub fn push(&mut self, pos: SectionPos) {
        if self.pending.insert(pos) {
            self.heap.push(Self::entry(self.target, pos));
        }
    }

    pub fn remove(&mut self, pos: SectionPos) {
        self.pending.remove(&pos);
    }

    pub fn contains(&self, pos: SectionPos) -> bool {
        self.pending.contains(&pos)
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Pop up to `max` sections, those nearest the load centre column first.
    /// Meshing is idempotent, so the order is a priority, not a contract.
    ///
    /// The heap is rebuilt only when the quantized load target changes. Ordinary
    /// frames pop `O(max log d)` work without copying or scanning the backlog.
    fn pop_nearest_batch(&mut self, max: usize, target: Option<LoadTarget>) -> Vec<SectionPos> {
        if max == 0 || self.pending.is_empty() {
            return Vec::new();
        }
        if self.target != target || self.heap.len() > self.pending.len().saturating_mul(4) + 1024 {
            self.rebuild(target);
        }
        let mut result = Vec::with_capacity(max.min(self.pending.len()));
        while result.len() < max {
            let Some(Reverse((_, cx, cy, cz))) = self.heap.pop() else {
                break;
            };
            let pos = SectionPos::new(cx, cy, cz);
            if self.pending.remove(&pos) {
                result.push(pos);
            }
        }
        result
    }
}
