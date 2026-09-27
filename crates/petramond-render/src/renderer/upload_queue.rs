use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use petramond_world::chunk::ChunkPos;

pub(super) const QUIET_FRAMES: u64 = 1;
pub(super) const MAX_WAIT_FRAMES: u64 = 4;

pub(super) const MAX_UPLOADS_PER_FRAME: usize = 64;
const MAX_ATTEMPTS_PER_FRAME: usize = 2 * MAX_UPLOADS_PER_FRAME;
const MAX_POPS_PER_FRAME: usize = 4 * MAX_UPLOADS_PER_FRAME;

pub(super) type UploadPriority = (u8, u32);

type UploadKey = (u8, u32, i32, i32, u64);

struct Pending {
    revision: u64,
    quiet_after: u64,
    deadline: u64,
}

#[derive(Default)]
pub(super) struct UploadQueue {
    pending: HashMap<ChunkPos, Pending>,
    heap: BinaryHeap<Reverse<UploadKey>>,
    frame: u64,
}

#[derive(Default)]
pub(super) struct FrameDrain {
    pops: usize,
    attempts: usize,
    uploads: usize,
    deferred: Vec<(ChunkPos, u64)>,
}

impl FrameDrain {
    pub(super) fn uploads(&self) -> usize {
        self.uploads
    }

    pub(super) fn record_upload(&mut self) {
        self.uploads += 1;
    }

    pub(super) fn defer(&mut self, column: ChunkPos, revision: u64) {
        self.deferred.push((column, revision));
    }
}

impl UploadQueue {
    pub(super) fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub(super) fn clear(&mut self) {
        self.pending.clear();
        self.heap.clear();
    }

    fn push(&mut self, column: ChunkPos, revision: u64, (hidden, distance): UploadPriority) {
        self.heap
            .push(Reverse((hidden, distance, column.cx, column.cz, revision)));
    }

    pub(super) fn mark_dirty(
        &mut self,
        column: ChunkPos,
        revision: u64,
        priority: impl FnOnce() -> UploadPriority,
    ) {
        let quiet_after = self.frame + QUIET_FRAMES;
        match self.pending.get_mut(&column) {
            Some(pending) if pending.revision == revision => return,
            Some(pending) => {
                pending.revision = revision;
                pending.quiet_after = quiet_after;
            }
            None => {
                self.pending.insert(
                    column,
                    Pending {
                        revision,
                        quiet_after,
                        deadline: self.frame + MAX_WAIT_FRAMES,
                    },
                );
            }
        }
        self.push(column, revision, priority());
    }

    pub(super) fn mark_urgent(
        &mut self,
        column: ChunkPos,
        priority: impl FnOnce() -> UploadPriority,
    ) {
        let frame = self.frame;
        let Some(pending) = self.pending.get_mut(&column) else {
            return;
        };
        pending.quiet_after = frame;
        pending.deadline = pending.deadline.min(frame);
        let revision = pending.revision;
        self.push(column, revision, priority());
    }

    pub(super) fn next_ready(&mut self, drain: &mut FrameDrain) -> Option<(ChunkPos, u64)> {
        while drain.attempts < MAX_ATTEMPTS_PER_FRAME
            && drain.pops < MAX_POPS_PER_FRAME
            && drain.uploads < MAX_UPLOADS_PER_FRAME
        {
            let Reverse((_, _, cx, cz, revision)) = self.heap.pop()?;
            drain.pops += 1;
            let column = ChunkPos::new(cx, cz);
            let Some(pending) = self.pending.get(&column) else {
                continue;
            };
            if pending.revision != revision {
                continue;
            }
            drain.attempts += 1;
            if self.frame < pending.quiet_after && self.frame < pending.deadline {
                drain.defer(column, revision);
                continue;
            }
            return Some((column, revision));
        }
        None
    }

    pub(super) fn take(&mut self, column: ChunkPos) {
        self.pending.remove(&column);
    }

    pub(super) fn restart(&mut self, column: ChunkPos, revision: u64, drain: &mut FrameDrain) {
        self.pending.insert(
            column,
            Pending {
                revision,
                quiet_after: self.frame + QUIET_FRAMES,
                deadline: self.frame + MAX_WAIT_FRAMES,
            },
        );
        drain.defer(column, revision);
    }

    pub(super) fn finish(
        &mut self,
        drain: FrameDrain,
        mut priority: impl FnMut(ChunkPos) -> UploadPriority,
    ) {
        for (column, revision) in drain.deferred {
            if self
                .pending
                .get(&column)
                .is_some_and(|pending| pending.revision == revision)
            {
                self.push(column, revision, priority(column));
            }
        }
    }
}

#[cfg(test)]
mod tests;
