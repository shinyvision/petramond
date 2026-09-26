//! The terrain upload scheduler: which dirty columns go to the GPU this
//! frame, and in what order.
//!
//! World dirtiness is level-triggered — a column stays dirty until it is
//! uploaded — so the queue deduplicates columns in `pending` while a heap
//! keeps each one's first useful priority. A column uploads once it has been
//! quiet (no new revision) for [`QUIET_FRAMES`], which coalesces a burst of
//! sibling-section completions into one repack, but never later than
//! [`MAX_WAIT_FRAMES`] after it first went dirty. Pure bookkeeping over plain
//! data: the GPU work stays with `sync_meshes`, so the scheduling is tested
//! without a device.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use petramond_world::chunk::ChunkPos;

/// Frames a column must go without a new revision before it uploads.
pub(super) const QUIET_FRAMES: u64 = 1;
/// Frames after first going dirty that a column uploads regardless.
pub(super) const MAX_WAIT_FRAMES: u64 = 4;

/// Max terrain columns uploaded per frame. CPU meshes stay section-owned, but
/// render-side buffers are packed per XZ column, so one upload can refresh
/// many vertical section ranges; excess stays queued for later frames.
///
/// The caller's TIME budget is the real frame guard; this count is a backstop
/// against a burst of individually-cheap uploads. The old cap of 6 (~360
/// columns/s) admission-limited fresh-terrain visibility during RD32 flight
/// (~200 fresh columns/s plus 2–3 re-uploads each while filling) with most of
/// the time budget unspent. Since the mesh workers seal sections into the GPU
/// vertex format, an upload is byte copies into staging memory (no
/// quantising, no concatenation), so the backstop sits well above what the
/// time budget admits in practice.
pub(super) const MAX_UPLOADS_PER_FRAME: usize = 64;
/// Current entries examined per frame (uploaded or deferred).
const MAX_ATTEMPTS_PER_FRAME: usize = 2 * MAX_UPLOADS_PER_FRAME;
/// Heap entries popped per frame, stale ones included.
const MAX_POPS_PER_FRAME: usize = 4 * MAX_UPLOADS_PER_FRAME;

/// A column's place in the upload order: `(hidden, distance bits)` — columns
/// in view soon before hidden ones, then nearest first. Equal priorities fall
/// back to the column position, so the order is a function of the scene.
pub(super) type UploadPriority = (u8, u32);

/// A heap entry: the priority, the column, and the revision it was queued at
/// (an entry whose revision is no longer the pending one is stale).
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

/// One frame's walk of the queue: its budget counters and the columns it
/// put off to a later frame.
#[derive(Default)]
pub(super) struct FrameDrain {
    pops: usize,
    attempts: usize,
    uploads: usize,
    deferred: Vec<(ChunkPos, u64)>,
}

impl FrameDrain {
    /// Columns uploaded so far this frame.
    pub(super) fn uploads(&self) -> usize {
        self.uploads
    }

    pub(super) fn record_upload(&mut self) {
        self.uploads += 1;
    }

    /// Put `column` off to a later frame (it stays pending).
    pub(super) fn defer(&mut self, column: ChunkPos, revision: u64) {
        self.deferred.push((column, revision));
    }
}

impl UploadQueue {
    /// Start a frame: the quiet windows and deadlines count these.
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

    /// `column` is dirty at `revision`. A new column queues with a fresh
    /// quiet window and deadline; a new revision of a queued one restarts its
    /// quiet window (not its deadline) and queues again; the same revision
    /// again changes nothing.
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

    /// The player just acted on `column`: skip its quiet window and deadline
    /// wait, so it uploads this frame. One more frame of coalescing would be
    /// visible latency on the thing they just did.
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

    /// The next column to upload this frame, in priority order, or `None`
    /// once the queue or the frame's budget runs out. Stale heap entries are
    /// skipped; a column still inside its quiet window (and before its
    /// deadline) is deferred into `drain`. The returned column stays pending
    /// until the caller [`take`](Self::take)s it.
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

    /// Stop tracking `column`: it is being uploaded (or has nothing left to
    /// upload).
    pub(super) fn take(&mut self, column: ChunkPos) {
        self.pending.remove(&column);
    }

    /// Hold `column` back with a fresh quiet window and deadline from this
    /// frame — while released sibling geometry is re-meshed, say — and put it
    /// off in `drain`.
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

    /// End the frame's walk: queue the deferred columns again, those still
    /// pending at the revision they were deferred with.
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
