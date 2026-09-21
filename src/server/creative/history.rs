//! A session's bounded undo/redo record of the cells its edits overwrote.

use crate::{schematic::ResolvedCell, world::cells::Cells};
use petramond_math::math::IVec3;
use std::collections::VecDeque;

const MAX_EDITS: usize = 16;
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// Both sides of one edit. A replay installs a side verbatim and never
/// rewrites either from the intervening world, so an edit replays the same
/// however the terrain changed in between.
pub(super) struct EditRecord {
    pub before: Cells,
    pub after: Cells,
    /// Bounds whose blocks re-run their updates when the edit is reapplied.
    pub update_bounds: Option<[IVec3; 2]>,
    bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Replay {
    Undo,
    Redo,
}

impl Replay {
    pub fn verb(self) -> &'static str {
        match self {
            Replay::Undo => "undo",
            Replay::Redo => "redo",
        }
    }
}

impl EditRecord {
    /// The cells a replay installs. They are lent to the edit and handed
    /// back through [`EditRecord::put_side`], so a replay copies nothing.
    pub fn take_side(&mut self, replay: Replay) -> Cells {
        std::mem::take(match replay {
            Replay::Undo => &mut self.before,
            Replay::Redo => &mut self.after,
        })
    }

    pub fn put_side(&mut self, replay: Replay, cells: Cells) {
        match replay {
            Replay::Undo => self.before = cells,
            Replay::Redo => self.after = cells,
        }
    }
}

#[derive(Default)]
pub struct EditHistory {
    undo: VecDeque<EditRecord>,
    redo: VecDeque<EditRecord>,
    /// Cells touched since the last [`EditHistory::close`], as they were
    /// before the first touch.
    open: Cells,
}

impl EditHistory {
    /// Note a cell about to change. Only its first state since the last
    /// close is kept, so everything touched in between becomes one edit.
    pub fn touch(&mut self, pos: IVec3, before: impl FnOnce() -> Option<ResolvedCell>) {
        if !self.open.iter().any(|(p, _)| *p == pos) {
            if let Some(before) = before() {
                self.open.push((pos, before));
            }
        }
    }

    pub fn has_open(&self) -> bool {
        !self.open.is_empty()
    }

    /// Close the touched cells into one edit; `after` reads a cell's present
    /// state. Cells that ended where they began are not an edit.
    pub fn close(&mut self, mut after: impl FnMut(IVec3) -> Option<ResolvedCell>) {
        let (before, after): (Cells, Cells) = std::mem::take(&mut self.open)
            .into_iter()
            .filter_map(|(pos, before)| {
                let after = after(pos)?;
                (after != before).then_some(((pos, before), (pos, after)))
            })
            .unzip();
        if !before.is_empty() {
            self.record(before, after, None);
        }
    }

    /// Enter a completed edit. It discards the redo branch.
    pub fn record(&mut self, before: Cells, after: Cells, update_bounds: Option<[IVec3; 2]>) {
        self.redo.clear();
        let bytes = retained_bytes(&before) + retained_bytes(&after);
        self.undo.push_back(EditRecord {
            before,
            after,
            update_bounds,
            bytes,
        });
        while self.undo.len() > 1
            && (self.undo.len() > MAX_EDITS
                || self.undo.iter().map(|e| e.bytes).sum::<usize>() > MAX_BYTES)
        {
            self.undo.pop_front();
        }
    }

    /// Lift the next record to replay out of its stack.
    pub(super) fn take(&mut self, replay: Replay) -> Option<EditRecord> {
        match replay {
            Replay::Undo => self.undo.pop_back(),
            Replay::Redo => self.redo.pop_back(),
        }
    }

    /// File a lifted record: on the opposite stack once replayed, back where
    /// it came from when the replay did not happen.
    pub(super) fn settle(&mut self, record: EditRecord, replay: Replay, replayed: bool) {
        match (replay, replayed) {
            (Replay::Undo, true) | (Replay::Redo, false) => self.redo.push_back(record),
            (Replay::Undo, false) | (Replay::Redo, true) => self.undo.push_back(record),
        }
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
}

/// Retained storage, counted without converting records to their portable form.
fn retained_bytes(cells: &Cells) -> usize {
    std::mem::size_of_val(cells.as_slice())
        + cells
            .iter()
            .map(|(_, data)| {
                data.kv
                    .iter()
                    .map(|(key, value)| {
                        std::mem::size_of::<(String, Vec<u8>, [usize; 3])>()
                            + key.capacity()
                            + value.capacity()
                    })
                    .sum::<usize>()
                    + data.container.as_ref().map_or(0, |c| {
                        c.slots.capacity()
                            * std::mem::size_of::<Option<petramond_world::item::ItemStack>>()
                    })
            })
            .sum::<usize>()
}
