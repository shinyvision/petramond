//! The loaded-section store, indexed by column: one hash lookup per XZ column and a direct
//! slot index for the section's `cy`. Every hot path that walks a 3x3x3 neighbourhood, a
//! column stack or the cells above/below a position pays one probe per COLUMN instead of one
//! per section, and a column probe hashes 8 bytes instead of 12.
//!
//! The API mirrors the `FxHashMap<SectionPos, Arc<Section>>` it replaced (`get`, `insert`,
//! `remove`, `iter`, ...), so readers and writers are unchanged; the column view
//! ([`SectionMap::column`], [`ColumnSlots::at`]) is the addition the hot paths use.

use std::sync::Arc;

use crate::chunk::{ChunkPos, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY};
use crate::section::Section;
use rustc_hash::FxHashMap;

pub const COLUMN_SLOTS: usize = (SECTION_MAX_CY - SECTION_MIN_CY + 1) as usize;

type Slot = Option<(SectionPos, Arc<Section>)>;

pub struct ColumnSlots {
    slots: Box<[Slot; COLUMN_SLOTS]>,
    count: u32,
}

#[inline]
fn slot_of(cy: i32) -> Option<usize> {
    let i = cy.wrapping_sub(SECTION_MIN_CY);
    ((i as u32 as usize) < COLUMN_SLOTS).then_some(i as usize)
}

impl ColumnSlots {
    fn empty() -> Self {
        Self {
            slots: Box::new(std::array::from_fn(|_| None)),
            count: 0,
        }
    }

    /// The section at `cy` of this column.
    #[inline]
    pub fn at(&self, cy: i32) -> Option<&Arc<Section>> {
        let i = slot_of(cy)?;
        self.slots[i].as_ref().map(|(_, s)| s)
    }

    #[inline]
    pub fn contains(&self, cy: i32) -> bool {
        slot_of(cy).is_some_and(|i| self.slots[i].is_some())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&SectionPos, &Arc<Section>)> {
        self.slots.iter().flatten().map(|(p, s)| (p, s))
    }
}

#[derive(Default)]
pub struct SectionMap {
    columns: FxHashMap<ChunkPos, ColumnSlots>,
    len: usize,
}

impl SectionMap {
    #[inline]
    pub fn column(&self, cp: ChunkPos) -> Option<&ColumnSlots> {
        self.columns.get(&cp)
    }

    #[inline]
    pub fn get(&self, pos: &SectionPos) -> Option<&Arc<Section>> {
        self.columns.get(&pos.chunk_pos())?.at(pos.cy)
    }

    #[inline]
    pub fn get_mut(&mut self, pos: &SectionPos) -> Option<&mut Arc<Section>> {
        let i = slot_of(pos.cy)?;
        self.columns.get_mut(&pos.chunk_pos())?.slots[i]
            .as_mut()
            .map(|(_, s)| s)
    }

    #[inline]
    pub fn contains_key(&self, pos: &SectionPos) -> bool {
        self.columns
            .get(&pos.chunk_pos())
            .is_some_and(|c| c.contains(pos.cy))
    }

    pub fn insert(&mut self, pos: SectionPos, section: Arc<Section>) -> Option<Arc<Section>> {
        let i = slot_of(pos.cy)?;
        let column = self
            .columns
            .entry(pos.chunk_pos())
            .or_insert_with(ColumnSlots::empty);
        let old = column.slots[i].replace((pos, section)).map(|(_, s)| s);
        if old.is_none() {
            column.count += 1;
            self.len += 1;
        }
        old
    }

    pub fn remove(&mut self, pos: &SectionPos) -> Option<Arc<Section>> {
        let i = slot_of(pos.cy)?;
        let cp = pos.chunk_pos();
        let column = self.columns.get_mut(&cp)?;
        let old = column.slots[i].take().map(|(_, s)| s);
        if old.is_some() {
            column.count -= 1;
            self.len -= 1;
            if column.count == 0 {
                self.columns.remove(&cp);
            }
        }
        old
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.columns.clear();
        self.len = 0;
    }

    pub fn iter(&self) -> impl Iterator<Item = (&SectionPos, &Arc<Section>)> {
        self.columns.values().flat_map(ColumnSlots::iter)
    }

    pub fn keys(&self) -> impl Iterator<Item = &SectionPos> {
        self.iter().map(|(p, _)| p)
    }

    pub fn values(&self) -> impl Iterator<Item = &Arc<Section>> {
        self.iter().map(|(_, s)| s)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Arc<Section>> {
        self.columns
            .values_mut()
            .flat_map(|c| c.slots.iter_mut().flatten().map(|(_, s)| s))
    }
}

impl<'a> IntoIterator for &'a SectionMap {
    type Item = (&'a SectionPos, &'a Arc<Section>);
    type IntoIter = Box<dyn Iterator<Item = (&'a SectionPos, &'a Arc<Section>)> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

impl std::ops::Index<&SectionPos> for SectionMap {
    type Output = Arc<Section>;

    fn index(&self, pos: &SectionPos) -> &Arc<Section> {
        self.get(pos).expect("section is loaded")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove_track_columns_and_len() {
        let mut map = SectionMap::default();
        let a = SectionPos::new(3, SECTION_MIN_CY, -2);
        let b = SectionPos::new(3, SECTION_MAX_CY, -2);
        let c = SectionPos::new(4, 0, -2);
        assert!(map.insert(a, Arc::new(Section::new(3, a.cy, -2))).is_none());
        assert!(map.insert(b, Arc::new(Section::new(3, b.cy, -2))).is_none());
        assert!(map.insert(c, Arc::new(Section::new(4, 0, -2))).is_none());
        assert_eq!(map.len(), 3);
        assert!(map.contains_key(&a) && map.contains_key(&b) && map.contains_key(&c));
        assert!(!map.contains_key(&SectionPos::new(3, 0, -2)));
        assert!(
            map.insert(a, Arc::new(Section::new(3, a.cy, -2))).is_some(),
            "replacing keeps the count"
        );
        assert_eq!(map.len(), 3);
        let column = map.column(ChunkPos::new(3, -2)).expect("column");
        assert_eq!(column.iter().count(), 2);
        assert!(
            column.at(SECTION_MAX_CY + 1).is_none(),
            "out of range reads as absent"
        );
        assert_eq!(map.keys().count(), 3);
        assert!(map.remove(&a).is_some());
        assert!(map.remove(&a).is_none());
        assert!(map.remove(&b).is_some());
        assert!(
            map.column(ChunkPos::new(3, -2)).is_none(),
            "an emptied column is dropped"
        );
        assert_eq!(map.len(), 1);
        map.clear();
        assert!(map.is_empty());
    }
}
