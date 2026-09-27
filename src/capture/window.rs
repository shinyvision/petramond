//! The stated world and the resident world.
//!
//! A presentation states more than it keeps loaded. The replica (the
//! RESIDENT world) holds every stated column inside the load window around
//! the presented camera, whole: its column state and every section stated
//! in it. Every other stated column is AWAY: an index entry naming where
//! each of its keys' content lives, a piece range in a mod file or, when
//! events changed it after it was last stated, the decoded content itself
//! (HELD). Terrain events released while a column is away wait on it, in
//! order, and apply when its content is next needed.
//!
//! Residency is per column, not per section, because a block write reads
//! the sections below it to move its column's heightmap: a column is
//! presented whole or not at all, so a write lands exactly as it would on a
//! replica holding everything stated.

use std::collections::BTreeMap;
use std::sync::Arc;

use rustc_hash::FxHashMap;

use petramond_world::chunk::{ChunkPos, SectionPos};

use crate::net::protocol::ColumnPayload;
use crate::world::{PieceRange, SectionContent, TerrainEdit};

#[derive(Clone, Debug)]
pub enum Away<T> {
    Range(PieceRange),
    Held(T),
}

impl<T> Away<T> {
    pub fn range(&self) -> Option<PieceRange> {
        match self {
            Away::Range(r) => Some(*r),
            Away::Held(_) => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AwayColumn {
    pub column: Option<Away<Arc<ColumnPayload>>>,
    pub sections: BTreeMap<i32, Away<SectionContent>>,
    pub pending: Vec<TerrainEdit>,
}

impl AwayColumn {
    pub fn presented_keys(&self) -> (bool, std::collections::BTreeSet<i32>) {
        let mut column = self.column.is_some();
        let mut sections: std::collections::BTreeSet<i32> = self.sections.keys().copied().collect();
        for edit in &self.pending {
            match edit {
                TerrainEdit::Section(p, _) => {
                    sections.insert(p.pos.cy);
                }
                TerrainEdit::Column(..) => column = true,
                TerrainEdit::SectionUnload(p) => {
                    sections.remove(&p.cy);
                }
                TerrainEdit::ColumnUnload(_) => {
                    column = false;
                    sections.clear();
                }
                TerrainEdit::Light(_) | TerrainEdit::Tick { .. } => {}
            }
        }
        (column, sections)
    }

    pub fn ranges(&self) -> impl Iterator<Item = PieceRange> + '_ {
        self.column
            .iter()
            .filter_map(Away::range)
            .chain(self.sections.values().filter_map(Away::range))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub center: ChunkPos,
    pub radius: i32,
}

impl Window {
    pub fn contains(&self, pos: ChunkPos) -> bool {
        let (dx, dz) = (
            i64::from(pos.cx - self.center.cx),
            i64::from(pos.cz - self.center.cz),
        );
        let r = i64::from(self.radius);
        dx * dx + dz * dz <= r * r
    }

    pub fn distance(&self, pos: ChunkPos) -> i64 {
        let (dx, dz) = (
            i64::from(pos.cx - self.center.cx),
            i64::from(pos.cz - self.center.cz),
        );
        dx * dx + dz * dz
    }
}

#[derive(Default)]
pub struct Stated {
    pub away: FxHashMap<ChunkPos, AwayColumn>,
}

impl Stated {
    pub fn has_section(&self, pos: SectionPos) -> bool {
        self.away
            .get(&pos.chunk_pos())
            .is_some_and(|c| c.presented_keys().1.contains(&pos.cy))
    }

    pub fn entry(&mut self, pos: ChunkPos) -> &mut AwayColumn {
        self.away.entry(pos).or_default()
    }

    pub fn forget_bytes(&mut self, incarnation: u64, start: u64, end: u64) -> usize {
        let hit = |r: &PieceRange| {
            r.incarnation == incarnation && r.offset < end && start < r.offset + r.len
        };
        let mut gone = 0;
        self.away.retain(|_, col| {
            if col
                .column
                .as_ref()
                .and_then(Away::range)
                .is_some_and(|r| hit(&r))
            {
                col.column = None;
                gone += 1;
            }
            col.sections.retain(|_, s| {
                let keep = !s.range().is_some_and(|r| hit(&r));
                gone += usize::from(!keep);
                keep
            });
            col.column.is_some() || !col.sections.is_empty()
        });
        gone
    }
}
