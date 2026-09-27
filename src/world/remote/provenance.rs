//! Where a presented replica's terrain came from, and what it replaced.
//!
//! A presentation installs sections and columns from pieces in a mod's
//! files. Each installed key remembers its ORIGIN, the piece range it holds
//! exactly, until a write changes it: every replica write seam clears the
//! origin of what it writes. A later apply passing that same range is then
//! skipped unread.
//!
//! Whatever replaces or unloads a key whose origin is a range keeps the
//! content it replaced in the PIECE CACHE, keyed by that range. The bytes
//! behind a range never change while the range is valid, so the cache is
//! never stale; it is dropped exactly where the bytes behind it change
//! ([`ReplicaWorld::invalidate_piece_bytes`]) or its file's incarnation ends.

use std::collections::VecDeque;
use std::sync::Arc;

use rustc_hash::FxHashMap;

use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_world::section::Section;

use crate::world::replication::{BlockDrawEntry, ColumnPayload};
use crate::world::ReplicaWorld;

/// One piece's bytes in one incarnation of a mod file.
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

/// A terrain key a replica holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Resident {
    Section(SectionPos),
    Column(ChunkPos),
}

/// A section as an install writes it: the section and its whole draw state.
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

/// Decoded terrain the cache keeps.
#[derive(Clone, Debug)]
pub enum Cached {
    Section(SectionContent),
    Column(Arc<ColumnPayload>),
}

#[derive(Default)]
pub(in crate::world) struct Provenance {
    by_range: FxHashMap<PieceRange, Resident>,
    by_resident: FxHashMap<Resident, PieceRange>,
    /// The origins of the installs about to run through the ordinary ingest,
    /// in order (`None`: that install states no piece).
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
    /// Track origins and keep replaced pieces (a presentation's replica, and
    /// the detached worlds its applies fold in).
    pub fn keep_provenance(&mut self) {
        if self.side.provenance.is_none() {
            self.side.provenance = Some(Box::default());
        }
    }

    /// The origins of the next installs the ordinary ingest runs, in order.
    pub fn expect_origins(&mut self, origins: impl IntoIterator<Item = Option<PieceRange>>) {
        if let Some(p) = self.side.provenance.as_mut() {
            p.pending.extend(origins);
        }
    }

    /// Forget origins no install consumed (a message the ingest dropped).
    pub fn clear_expected_origins(&mut self) {
        if let Some(p) = self.side.provenance.as_mut() {
            p.pending.clear();
        }
    }

    /// The piece range `resident` holds exactly, while nothing wrote it since.
    pub fn origin_of(&self, resident: Resident) -> Option<PieceRange> {
        self.side
            .provenance
            .as_ref()?
            .by_resident
            .get(&resident)
            .copied()
    }

    /// Decoded content the cache holds for `range`.
    pub fn cached_piece(&self, range: &PieceRange) -> Option<&Cached> {
        self.side.provenance.as_ref()?.cache.get(range)
    }

    /// Keep decoded content for `range` (a piece read or released).
    pub fn cache_piece(&mut self, range: PieceRange, content: Cached) {
        if let Some(p) = self.side.provenance.as_mut() {
            p.cache.insert(range, content);
        }
    }

    /// Drop the cache's copy of `range`: the replica now holds that content
    /// itself (so marking it for a remesh never copies it), and keeps it again
    /// when something replaces it.
    pub fn uncache_piece(&mut self, range: &PieceRange) {
        if let Some(p) = self.side.provenance.as_mut() {
            p.cache.remove(range);
        }
    }

    /// How many pieces the cache holds.
    pub fn cached_pieces(&self) -> usize {
        self.side.provenance.as_ref().map_or(0, |p| p.cache.len())
    }

    /// The bytes `[start, end)` of incarnation `incarnation` changed: every
    /// origin and cache entry they overlap is dropped, nothing else.
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

    /// `resident` holds exactly the piece at `origin` (`None`: no piece).
    pub(in crate::world) fn set_origin(&mut self, resident: Resident, origin: Option<PieceRange>) {
        if let Some(p) = self.side.provenance.as_mut() {
            p.set(resident, origin);
        }
    }

    /// The next ordinary install's origin, taken whether or not it installs.
    pub(in crate::world) fn take_record_origin(&mut self) -> Option<PieceRange> {
        self.side.provenance.as_mut()?.pending.pop_front().flatten()
    }

    /// A write is about to change section `pos`: it no longer holds what any
    /// piece states, and what it held is kept under the range it came from.
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

    /// [`before_section_write`](Self::before_section_write) for a column.
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
