use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

use rustc_hash::FxHashSet;

use crate::mathh::IVec3;

type ScheduledTick = Reverse<(u64, u64, i32, i32, i32)>;

#[derive(Default)]
pub struct ScheduledQueue {
    heap: BinaryHeap<ScheduledTick>,
    seq: u64,
    pending: FxHashSet<IVec3>,
}

impl ScheduledQueue {
    pub fn schedule(&mut self, pos: IVec3, due: u64) -> bool {
        if !self.pending.insert(pos) {
            return false;
        }
        let seq = self.seq;
        self.seq += 1;
        self.heap.push(Reverse((due, seq, pos.x, pos.y, pos.z)));
        true
    }

    pub fn pop_due(&mut self, now: u64) -> Option<IVec3> {
        let &Reverse((due, _, x, y, z)) = self.heap.peek()?;
        if due > now {
            return None;
        }
        self.heap.pop();
        let pos = IVec3::new(x, y, z);
        self.pending.remove(&pos);
        Some(pos)
    }

    pub fn contains(&self, pos: IVec3) -> bool {
        self.pending.contains(&pos)
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    pub fn due_count(&self, now: u64) -> usize {
        self.heap
            .iter()
            .filter(|Reverse((due, ..))| *due <= now)
            .count()
    }
}

#[derive(Default)]
pub struct TickState {
    pub tick: u64,
    pub update_queue: VecDeque<IVec3>,
    pub update_set: FxHashSet<IVec3>,
    pub scheduled: ScheduledQueue,
    pub fluid: ScheduledQueue,
    pub pending_breaks: Vec<(IVec3, crate::block::Block)>,
    pub rng: u64,
    pub batch_scratch: Vec<IVec3>,
    pub change_log: ChangeLog,
}

pub const CHANGE_LOG_CAP: usize = 4096;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub pos: IVec3,
    pub nav: bool,
}

#[derive(Default)]
pub struct ChangeLog {
    window: VecDeque<Change>,
    base: u64,
    nav_revision: u64,
}

impl ChangeLog {
    pub fn push(&mut self, pos: IVec3, nav: bool) {
        if self.window.len() >= CHANGE_LOG_CAP {
            self.window.pop_front();
            self.base += 1;
        }
        self.window.push_back(Change { pos, nav });
        if nav {
            self.nav_revision = self.nav_revision.wrapping_add(1);
        }
    }

    pub fn end(&self) -> u64 {
        self.base + self.window.len() as u64
    }

    pub fn nav_revision(&self) -> u64 {
        self.nav_revision
    }

    pub fn since(&self, seq: u64) -> (Vec<IVec3>, bool) {
        self.collect_since(seq, |_| true)
    }

    pub fn nav_since(&self, seq: u64) -> (Vec<IVec3>, bool) {
        self.collect_since(seq, |c| c.nav)
    }

    fn collect_since(&self, seq: u64, keep: impl Fn(&Change) -> bool) -> (Vec<IVec3>, bool) {
        if seq < self.base || seq > self.end() {
            return (Vec::new(), true);
        }
        let start = (seq - self.base) as usize;
        let cells = self
            .window
            .range(start..)
            .filter(|c| keep(c))
            .map(|c| c.pos);
        (cells.collect(), false)
    }
}

#[cfg(test)]
mod tests;

impl TickState {
    pub fn new(seed: u32) -> Self {
        Self {
            rng: (seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            ..Default::default()
        }
    }

    #[inline]
    pub fn next_random(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }
}
