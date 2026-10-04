//! Which camps still fly their flag. While the flags pack is loaded, a camp's empty posts fill
//! only while a skull flag stands somewhere in the camp's box: taking it down ends the garrison,
//! and putting one up again anywhere in the camp brings it back.
//!
//! A camp's box is searched for flags when one of its posts is first due, piece by piece as the
//! pieces load; after that the world's change log keeps the camp's flags current.

use std::collections::{BTreeMap, BTreeSet};

use mod_sdk::*;

use crate::keys::SKULL_FLAG;
use crate::post_marker::CampBox;

/// Edge of the boxes a camp is searched in, within the host's search cap.
const PIECE: i32 = 32;

pub struct Standards {
    flag: BlockId,
    changes: ChangeCursor,
    camps: BTreeMap<CampBox, Camp>,
}

#[derive(Default)]
struct Camp {
    flags: BTreeSet<[i32; 3]>,
    /// The pieces not searched yet; `None` until the camp is first asked about.
    unsearched: Option<Vec<CampBox>>,
    searched_at: Option<u64>,
}

impl Standards {
    /// `None` without the flag block: camps fly no flags, and every post fills.
    pub fn init() -> Option<Standards> {
        Some(Standards {
            flag: resolve_block(SKULL_FLAG)?,
            changes: ChangeCursor::default(),
            camps: BTreeMap::new(),
        })
    }

    /// Follows the world's changes into the flags of the camps asked about so far.
    pub fn follow(&mut self) {
        let changes = self.changes.advance();
        if changes.lost {
            self.camps.values_mut().for_each(|c| *c = Camp::default());
            return;
        }
        let cells: Vec<[i32; 3]> = changes
            .cells
            .into_iter()
            .filter(|&c| self.camps.keys().any(|b| b.contains(c)))
            .collect();
        if cells.is_empty() {
            return;
        }
        let blocks = paged(cells.clone(), get_blocks);
        for (cell, block) in cells.into_iter().zip(blocks) {
            for (bounds, camp) in &mut self.camps {
                if !bounds.contains(cell) {
                    continue;
                }
                match block {
                    Some(b) if b == self.flag => {
                        camp.flags.insert(cell);
                    }
                    Some(_) => {
                        camp.flags.remove(&cell);
                    }
                    // Changed, then unloaded before it was read: search the camp again.
                    None => *camp = Camp::default(),
                }
            }
        }
    }

    /// Whether a flag is known to stand in the camp, searching (at most once a tick) whatever of
    /// it has not been searched yet.
    pub fn flies(&mut self, bounds: CampBox, now: u64) -> bool {
        let flag = self.flag;
        let Camp {
            flags,
            unsearched,
            searched_at,
        } = self.camps.entry(bounds).or_default();
        if !flags.is_empty() || *searched_at == Some(now) {
            return !flags.is_empty();
        }
        *searched_at = Some(now);
        unsearched
            .get_or_insert_with(|| pieces(bounds))
            .retain(
                |piece| match find_blocks(piece.min, piece.max, vec![flag]) {
                    Some(found) => {
                        flags.extend(found);
                        false
                    }
                    None => true,
                },
            );
        !flags.is_empty()
    }
}

/// `bounds` cut into boxes of at most [`PIECE`] cells a side, lowest first.
pub fn pieces(bounds: CampBox) -> Vec<CampBox> {
    let starts = |a: usize| (bounds.min[a]..=bounds.max[a]).step_by(PIECE as usize);
    let mut out = Vec::new();
    for y in starts(1) {
        for z in starts(2) {
            for x in starts(0) {
                let min = [x, y, z];
                let max = [0, 1, 2].map(|a| (min[a] + PIECE - 1).min(bounds.max[a]));
                out.push(CampBox { min, max });
            }
        }
    }
    out
}
