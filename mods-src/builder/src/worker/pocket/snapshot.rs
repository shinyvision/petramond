use crate::fx::HashSet;
use crate::host::prelude::*;
use crate::worker::tuning::access::READ_CELLS;
use crate::worker::tuning::waits::UNREAD;

use super::grid::Bounds;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Sample {
    Unknown,
    Boundary,
    Block(BlockId),
}

pub(super) struct Snapshot {
    pub bounds: Bounds,
    pub samples: Vec<Sample>,
    pub revision: u64,
    cursor: usize,
    dirty: HashSet<usize>,
    retry_at: u64,
    complete: bool,
}

impl Snapshot {
    pub fn new(bounds: Bounds) -> Self {
        Self {
            bounds,
            samples: vec![Sample::Unknown; bounds.len()],
            revision: 0,
            cursor: 0,
            dirty: HashSet::default(),
            retry_at: 0,
            complete: false,
        }
    }

    pub fn changed(&mut self, cells: &[[i32; 3]], lost: bool) -> bool {
        if lost {
            self.cursor = 0;
            self.dirty.clear();
            self.complete = false;
            return true;
        }
        let before = self.dirty.len();
        self.dirty
            .extend(cells.iter().filter_map(|c| self.bounds.index(*c)));
        let changed = self.dirty.len() != before;
        if changed {
            self.complete = false;
        }
        changed
    }

    pub fn read(&mut self, now: u64) -> bool {
        if self.complete {
            return true;
        }
        let end = (self.cursor + READ_CELLS).min(self.samples.len());
        let mut indices: Vec<usize> = (self.cursor..end).collect();
        self.cursor = end;
        let mut dirty: Vec<usize> = self.dirty.iter().copied().collect();
        dirty.sort_unstable();
        for i in dirty.into_iter().take(READ_CELLS - indices.len()) {
            self.dirty.remove(&i);
            indices.push(i);
        }
        if indices.is_empty() && now >= self.retry_at {
            self.dirty.extend(
                self.samples
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| matches!(s, Sample::Unknown).then_some(i)),
            );
            self.retry_at = now + UNREAD;
            indices.extend(self.dirty.iter().copied());
            indices.sort_unstable();
            indices.truncate(READ_CELLS);
            for i in &indices {
                self.dirty.remove(i);
            }
        }
        let positions: Vec<_> = indices.iter().map(|i| self.bounds.cell(*i)).collect();
        let blocks = paged(positions.clone(), get_blocks);
        let mut unknown = Vec::new();
        for ((i, pos), block) in indices.into_iter().zip(positions).zip(blocks) {
            let sample = block.map_or(Sample::Unknown, Sample::Block);
            if self.samples[i] != sample {
                self.revision += 1;
                self.samples[i] = sample;
            }
            if block.is_none() {
                unknown.push((i, pos))
            }
        }
        let statuses = paged(
            unknown
                .iter()
                .map(|(_, pos)| (*pos, CellRecord::of(crate::keys::AIR)))
                .collect(),
            record_statuses,
        );
        for ((i, _), status) in unknown.into_iter().zip(statuses) {
            if matches!(status, RecordStatus::Unsupported { .. }) {
                if self.samples[i] != Sample::Boundary {
                    self.revision += 1;
                }
                self.samples[i] = Sample::Boundary;
            }
        }
        self.complete = self.cursor == self.samples.len()
            && self.dirty.is_empty()
            && !self.samples.iter().any(|s| matches!(s, Sample::Unknown));
        self.complete
    }
}
