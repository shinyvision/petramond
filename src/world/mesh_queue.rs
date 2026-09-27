use rustc_hash::FxHashSet;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

use petramond_world::chunk::SectionPos;

use super::store::LoadTarget;

mod cpu_release;
mod light_pump;
mod mesh_jobs;
mod prediction;
mod pump;
mod sealing;
pub(in crate::world) mod settle;

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
const RESULT_DRAIN_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(700);
const RESULT_DRAIN_MIN: usize = 24;
/// Cap on mesh jobs in flight in the shared pool. The pool queue is priority-ordered
/// (nearest first), so a fresh edit takes priority over streaming backlog.
/// This cap bounds snapshot memory held
/// by queued jobs and stale-priority momentum after a target move. The backlog beyond
/// it stays in `dirty_meshes`, re-sorted NEAREST-FIRST every frame.
///
/// Sized to keep the worker pool FED for a frame of bulk streaming (~4 jobs/worker at
/// ~1 ms/job, 60 fps) without limiting admission below the pool's capacity.
fn max_mesh_jobs_in_flight() -> usize {
    static CAP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CAP.get_or_init(|| (crate::worker::JobPool::default_threads() * 4).clamp(16, 256))
}
const MESH_SUBMIT_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(2_000);
pub(super) const MESH_RELEASE_DELAY_FRAMES: u64 = 600;
const MESH_RELEASE_SWEEP_INTERVAL: u64 = 64;

#[derive(Default)]
pub(super) struct DirtyMeshQueue {
    pending: FxHashSet<SectionPos>,
    heap: BinaryHeap<Reverse<(i64, i32, i32, i32)>>,
    target: Option<LoadTarget>,
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
