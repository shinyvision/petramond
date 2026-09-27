use std::collections::VecDeque;
use std::sync::Arc;

use rustc_hash::FxHashMap;

use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_world::section::Section;

use crate::world::replication::{BlockDrawEntry, ColumnPayload};
use crate::world::ReplicaWorld;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PieceRange {
    pub incarnation: u64,
    pub offset: u64,
    pub len: u64,
}

impl PieceRange {
    fn overlaps(&self, incarnation: u64, start: u64, end: u64) -> bool {
        self.incarnation == incarnation && self.offset < end && start < self.offset + self.len
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Resident {
    Section(SectionPos),
    Column(ChunkPos),
}

#[derive(Clone)]
pub struct SectionContent {
    pub section: Arc<Section>,
    pub draws: Vec<BlockDrawEntry>,
}

impl std::fmt::Debug for SectionContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = &self.section;
        write!(
            f,
            "SectionContent({}, {}, {}; {} draws)",
            s.cx,
            s.cy,
            s.cz,
            self.draws.len()
        )
    }
}

impl SectionContent {
    pub fn same(&self, other: &SectionContent) -> bool {
        (Arc::ptr_eq(&self.section, &other.section) || self.section.same_content(&other.section))
            && self.draws == other.draws
    }
}

#[derive(Clone, Debug)]
pub enum Cached {
    Section(SectionContent),
    Column(Arc<ColumnPayload>),
}

#[derive(Default)]
pub(in crate::world) struct Provenance {
    by_range: FxHashMap<PieceRange, Resident>,
    by_resident: FxHashMap<Resident, PieceRange>,
    pending: VecDeque<Option<PieceRange>>,
    cache: FxHashMap<PieceRange, Cached>,
}

impl Provenance {
    fn clear(&mut self, resident: Resident) -> Option<PieceRange> {
        let range = self.by_resident.remove(&resident)?;
        self.by_range.remove(&range);
        Some(range)
    }

    fn set(&mut self, resident: Resident, origin: Option<PieceRange>) {
        self.clear(resident);
        let Some(range) = origin else {
            return;
        };
        if let Some(stale) = self.by_range.insert(range, resident) {
            if stale != resident {
                self.by_resident.remove(&stale);
            }
        }
        self.by_resident.insert(resident, range);
    }
}

impl ReplicaWorld {
    pub fn keep_provenance(&mut self) {
        if self.side.provenance.is_none() {
            self.side.provenance = Some(Box::default());
        }
    }

    pub fn expect_origins(&mut self, origins: impl IntoIterator<Item = Option<PieceRange>>) {
        if let Some(p) = self.side.provenance.as_mut() {
            p.pending.extend(origins);
        }
    }

    pub fn clear_expected_origins(&mut self) {
        if let Some(p) = self.side.provenance.as_mut() {
            p.pending.clear();
        }
    }

    pub fn origin_of(&self, resident: Resident) -> Option<PieceRange> {
        self.side
            .provenance
            .as_ref()?
            .by_resident
            .get(&resident)
            .copied()
    }

    pub fn cached_piece(&self, range: &PieceRange) -> Option<&Cached> {
        self.side.provenance.as_ref()?.cache.get(range)
    }

    pub fn cache_piece(&mut self, range: PieceRange, content: Cached) {
        if let Some(p) = self.side.provenance.as_mut() {
            p.cache.insert(range, content);
        }
    }

    pub fn uncache_piece(&mut self, range: &PieceRange) {
        if let Some(p) = self.side.provenance.as_mut() {
            p.cache.remove(range);
        }
    }

    pub fn cached_pieces(&self) -> usize {
        self.side.provenance.as_ref().map_or(0, |p| p.cache.len())
    }

    pub fn invalidate_piece_bytes(&mut self, incarnation: u64, start: u64, end: u64) {
        let Some(p) = self.side.provenance.as_mut() else {
            return;
        };
        p.cache.retain(|r, _| !r.overlaps(incarnation, start, end));
        let stale: Vec<Resident> = p
            .by_range
            .iter()
            .filter(|(r, _)| r.overlaps(incarnation, start, end))
            .map(|(_, &k)| k)
            .collect();
        for k in stale {
            p.clear(k);
        }
    }

    pub(in crate::world) fn set_origin(&mut self, resident: Resident, origin: Option<PieceRange>) {
        if let Some(p) = self.side.provenance.as_mut() {
            p.set(resident, origin);
        }
    }

    pub(in crate::world) fn take_record_origin(&mut self) -> Option<PieceRange> {
        self.side.provenance.as_mut()?.pending.pop_front().flatten()
    }

    pub(in crate::world) fn before_section_write(&mut self, pos: SectionPos) {
        self.stamp_section_write(pos);
        let Some(range) = self
            .side
            .provenance
            .as_mut()
            .and_then(|p| p.clear(Resident::Section(pos)))
        else {
            return;
        };
        if let Some(section) = self.data.sections.get(&pos).cloned() {
            let draws = self.section_block_draws(pos);
            self.cache_piece(range, Cached::Section(SectionContent { section, draws }));
        }
    }

    pub(in crate::world) fn before_column_write(&mut self, pos: ChunkPos) {
        self.stamp_column_write(pos);
        let Some(range) = self
            .side
            .provenance
            .as_mut()
            .and_then(|p| p.clear(Resident::Column(pos)))
        else {
            return;
        };
        if let Some(payload) = self.column_content(pos) {
            self.cache_piece(range, Cached::Column(Arc::new(payload)));
        }
    }
}
