//! The random-tick scan's index: per column, which sections hold a random-tickable cell, and for
//! each such section a 4096-bit mask of exactly those cells.
//!
//! The scan draws three cells per tickable section and acts on the ones that are tickable. With
//! the mask it answers "is this cell tickable" from 512 bytes that stay cache-resident between
//! ticks, instead of a section hash lookup plus a read into a cold 4 KiB block cube per draw. A
//! mask is derived from the section's blocks on first use and remembers the block cube's stamp;
//! the index repair drops it when a touched section's cube carries another stamp (an edit or a
//! new install), so the answer is always the one the blocks give.

use crate::block::Block;
use crate::chunk::{SectionPos, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_VOLUME};
use crate::section::Section;

const SLOTS: usize = (SECTION_MAX_CY - SECTION_MIN_CY + 1) as usize;

pub struct TickMask([u64; SECTION_VOLUME / 64]);

impl TickMask {
    #[inline]
    pub fn get(&self, i: usize) -> bool {
        self.0[i >> 6] & (1 << (i & 63)) != 0
    }

    /// One bit per 64-cell word: set when the word holds a tickable cell.
    fn summary(&self) -> u64 {
        self.0
            .iter()
            .enumerate()
            .fold(0, |s, (w, &word)| s | (u64::from(word != 0) << w))
    }

    fn of(section: &Section, tickable: &TickableIds) -> TickMask {
        let mut words = [0u64; SECTION_VOLUME / 64];
        let blocks = section.blocks();
        match blocks.as_narrow() {
            Some(bytes) => {
                let lut = &tickable.narrow;
                for (word, cells) in words.iter_mut().zip(bytes.chunks_exact(64)) {
                    *word = cells
                        .iter()
                        .enumerate()
                        .fold(0u64, |w, (j, &id)| w | (u64::from(lut[id as usize]) << j));
                }
            }
            None => blocks.cells_where(|id| tickable.get(id), |i| words[i >> 6] |= 1 << (i & 63)),
        }
        TickMask(words)
    }
}

/// Which block ids random-tick, for the content registry it was built from.
pub struct TickableIds {
    serial: u64,
    ids: Vec<bool>,
    narrow: [u8; 256],
}

impl Default for TickableIds {
    fn default() -> Self {
        TickableIds {
            serial: 0,
            ids: Vec::new(),
            narrow: [0; 256],
        }
    }
}

impl TickableIds {
    fn refresh(&mut self) {
        let content = crate::content::Content::current();
        if self.serial == content.serial() && !self.ids.is_empty() {
            return;
        }
        let count = content.names().blocks.len().max(1);
        self.ids = (0..count)
            .map(|id| id != 0 && Block::from_id(id as u16).has_random_tick())
            .collect();
        let narrow: [u8; 256] = std::array::from_fn(|id| u8::from(self.get(id as u16)));
        self.narrow = narrow;
        self.serial = content.serial();
    }

    #[inline]
    fn get(&self, id: u16) -> bool {
        match self.ids.get(id as usize) {
            Some(&tickable) => tickable,
            None => id != 0 && Block::from_id(id).has_random_tick(),
        }
    }
}

/// A section's tickable cells as a word summary (kept inline, so a draw into an empty word is
/// answered without touching the mask) and the mask itself.
pub struct SectionTicks {
    summary: u64,
    stamp: u64,
    mask: Box<TickMask>,
}

impl SectionTicks {
    #[inline]
    pub fn get(&self, i: usize) -> bool {
        self.summary & (1 << (i >> 6)) != 0 && self.mask.get(i)
    }
}

#[derive(Default)]
pub struct RandomTickColumn {
    bits: u32,
    masks: [Option<SectionTicks>; SLOTS],
}

impl RandomTickColumn {
    #[inline]
    pub fn bits(&self) -> u32 {
        self.bits
    }

    /// Records whether the section in `slot` holds anything tickable and drops its mask unless
    /// it was derived from blocks carrying `stamp`.
    pub fn set(&mut self, slot: usize, tickable: bool, stamp: u64) {
        if tickable {
            self.bits |= 1 << slot;
        } else {
            self.bits &= !(1 << slot);
        }
        if !tickable || self.masks[slot].as_ref().is_some_and(|m| m.stamp != stamp) {
            self.masks[slot] = None;
        }
    }

    /// The mask of the tickable section in `slot`, derived from `section` if stale. `None` when
    /// the slot's section is not loaded.
    pub fn mask<'s>(
        &mut self,
        slot: usize,
        section: impl FnOnce() -> Option<&'s Section>,
        tickable: &TickableIds,
    ) -> Option<&SectionTicks> {
        if self.masks[slot].is_none() {
            let section = section()?;
            let mask = Box::new(TickMask::of(section, tickable));
            self.masks[slot] = Some(SectionTicks {
                summary: mask.summary(),
                stamp: section.blocks().stamp(),
                mask,
            });
        }
        self.masks[slot].as_ref()
    }
}

#[derive(Default)]
pub struct RandomTickIndex {
    pub columns: rustc_hash::FxHashMap<crate::chunk::ChunkPos, RandomTickColumn>,
    pub tickable: TickableIds,
}

impl RandomTickIndex {
    #[inline]
    pub fn slot(cy: i32) -> usize {
        (cy - SECTION_MIN_CY) as usize
    }

    /// `section` is the loaded section at `pos`, if any.
    pub fn note(&mut self, pos: SectionPos, section: Option<&Section>) {
        let column = pos.chunk_pos();
        let tickable = section.is_some_and(|s| s.has_random_tickable());
        let stamp = section.map_or(0, |s| s.blocks().stamp());
        if tickable {
            self.columns
                .entry(column)
                .or_default()
                .set(Self::slot(pos.cy), true, stamp);
        } else if let Some(entry) = self.columns.get_mut(&column) {
            entry.set(Self::slot(pos.cy), false, stamp);
            if entry.bits == 0 {
                self.columns.remove(&column);
            }
        }
    }

    pub fn refresh_ids(&mut self) {
        self.tickable.refresh();
    }
}
