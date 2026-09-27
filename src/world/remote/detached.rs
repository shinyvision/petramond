//! The replica's write functions over a DETACHED view: a scratch replica
//! seeded with the presented content of the columns a stretch of events
//! touches, as shared `Arc`s. Sections are copy-on-write, so a write copies
//! only the buffers it changes and the presented sections stay exactly as
//! they are; the fold's result is read back per key and compared.
//!
//! The same seams the live replica ingests through run here, in the same
//! order, so a stretch folded detached lands on exactly what ingesting it on
//! the presented replica would have left.

use std::sync::Arc;

use petramond_world::chunk::{ChunkPos, SectionPos};

use super::provenance::{PieceRange, Resident, SectionContent};
use crate::worker::JobPool;
use crate::world::replication::{
    BlockDelta, BlockDrawDelta, CellKvDelta, ColumnPayload, LightPayload, SectionPayload,
};
use crate::world::store::column_cy_bit;
use crate::world::ReplicaWorld;

/// One terrain write a replica applies, as the stream carries it.
#[derive(Clone, Debug)]
pub enum TerrainEdit {
    /// A section arriving in full, from the piece at the range when one states it.
    Section(Arc<SectionPayload>, Option<PieceRange>),
    Column(Arc<ColumnPayload>, Option<PieceRange>),
    Light(LightPayload),
    SectionUnload(SectionPos),
    ColumnUnload(ChunkPos),
    /// A tick batch's terrain writes, in the batch's order.
    Tick {
        deltas: Vec<BlockDelta>,
        draws: Vec<BlockDrawDelta>,
        kv: Vec<CellKvDelta>,
    },
}

impl TerrainEdit {
    /// The columns this edit writes.
    pub fn columns(&self, mut each: impl FnMut(ChunkPos)) {
        let cell = |p: petramond_math::math::IVec3| {
            SectionPos::from_world(p.x, p.y, p.z).map(|s| s.chunk_pos())
        };
        match self {
            Self::Section(p, _) => each(p.pos.chunk_pos()),
            Self::Column(p, _) => each(p.pos),
            Self::Light(p) => each(p.pos.chunk_pos()),
            Self::SectionUnload(p) => each(p.chunk_pos()),
            Self::ColumnUnload(p) => each(*p),
            Self::Tick { deltas, draws, kv } => {
                deltas
                    .iter()
                    .filter_map(|d| cell(d.pos))
                    .chain(draws.iter().filter_map(|d| cell(d.pos)))
                    .chain(kv.iter().filter_map(|d| cell(d.pos)))
                    .for_each(each);
            }
        }
    }

    /// The part of this edit that writes column `pos`, if any.
    pub fn for_column(&self, pos: ChunkPos) -> Option<TerrainEdit> {
        let within = |p: petramond_math::math::IVec3| {
            SectionPos::from_world(p.x, p.y, p.z).is_some_and(|s| s.chunk_pos() == pos)
        };
        match self {
            Self::Tick { deltas, draws, kv } => {
                let part = Self::Tick {
                    deltas: deltas.iter().filter(|d| within(d.pos)).cloned().collect(),
                    draws: draws.iter().filter(|d| within(d.pos)).cloned().collect(),
                    kv: kv.iter().filter(|d| within(d.pos)).cloned().collect(),
                };
                match &part {
                    Self::Tick { deltas, draws, kv }
                        if deltas.is_empty() && draws.is_empty() && kv.is_empty() =>
                    {
                        None
                    }
                    _ => Some(part),
                }
            }
            other => {
                let mut hit = false;
                other.columns(|c| hit |= c == pos);
                hit.then(|| other.clone())
            }
        }
    }
}

impl ReplicaWorld {
    /// A tick batch's terrain writes, in the one order every replica applies
    /// them: blocks first (a block write wipes the cell's draw set and KV on
    /// both sides), then draw sets, then cell KV.
    pub fn apply_remote_tick_terrain(
        &mut self,
        deltas: Vec<BlockDelta>,
        draws: Vec<BlockDrawDelta>,
        kv: Vec<CellKvDelta>,
    ) {
        for delta in deltas {
            self.apply_remote_delta(delta);
        }
        for d in draws {
            self.apply_remote_block_draw(d.pos, d.prims);
        }
        for d in kv {
            self.apply_remote_cell_kv(d);
        }
    }

    /// Apply one terrain edit through the ordinary ingest seams. Answers the
    /// section it installed in full, for the caller's install batch.
    pub fn apply_terrain_edit(&mut self, edit: TerrainEdit) -> Option<SectionPos> {
        match edit {
            TerrainEdit::Section(payload, origin) => {
                self.expect_origins([origin]);
                let payload = Arc::try_unwrap(payload).unwrap_or_else(|p| (*p).clone());
                let installed = self.install_remote_section_deferred(payload);
                self.clear_expected_origins();
                installed
            }
            TerrainEdit::Column(payload, origin) => {
                self.expect_origins([origin]);
                let payload = Arc::try_unwrap(payload).unwrap_or_else(|p| (*p).clone());
                self.install_remote_column(payload);
                self.clear_expected_origins();
                None
            }
            TerrainEdit::Light(light) => {
                self.install_remote_light(light);
                None
            }
            TerrainEdit::SectionUnload(pos) => {
                self.uninstall_remote_section(pos);
                None
            }
            TerrainEdit::ColumnUnload(pos) => {
                self.uninstall_remote_column(pos);
                None
            }
            TerrainEdit::Tick { deltas, draws, kv } => {
                self.apply_remote_tick_terrain(deltas, draws, kv);
                None
            }
        }
    }
}

/// A scratch replica a fold runs in. It holds only what it was seeded with.
pub struct DetachedFold {
    world: ReplicaWorld,
}

impl Default for DetachedFold {
    fn default() -> Self {
        Self::new()
    }
}

impl DetachedFold {
    pub fn new() -> Self {
        let mut world = ReplicaWorld::with_pool(0, 1, Arc::new(JobPool::inline()));
        world.keep_provenance();
        Self { world }
    }

    /// Seed column `payload` as presented, holding the piece at `origin`.
    pub fn seed_column(&mut self, payload: ColumnPayload, origin: Option<PieceRange>) {
        let pos = payload.pos;
        self.world.expect_origins([origin]);
        self.world.install_remote_column(payload);
        self.world.clear_expected_origins();
        self.world.set_origin(Resident::Column(pos), origin);
    }

    /// Seed section `pos` as presented: the presented `Arc` itself, shared,
    /// with its draws. No install work runs; nothing here is presented.
    pub fn seed_section(
        &mut self,
        pos: SectionPos,
        content: SectionContent,
        origin: Option<PieceRange>,
    ) {
        let w = &mut self.world;
        w.data.ensure_column(pos.chunk_pos());
        w.data.sections.insert(pos, content.section);
        *w.data
            .section_column_cys
            .entry(pos.chunk_pos())
            .or_insert(0) |= column_cy_bit(pos.cy);
        for (cell, prims) in content.draws {
            let (lx, ly, lz) = petramond_world::chunk::section_local(cell as usize);
            let at = petramond_math::math::IVec3::new(
                pos.cx * 16 + lx as i32,
                pos.cy * 16 + ly as i32,
                pos.cz * 16 + lz as i32,
            );
            w.apply_remote_block_draw(at, prims);
        }
        w.set_origin(Resident::Section(pos), origin);
    }

    pub fn apply(&mut self, edit: TerrainEdit) {
        self.world.apply_terrain_edit(edit);
    }

    pub fn has_column(&self, pos: ChunkPos) -> bool {
        self.world.data().columns.contains_key(&pos)
    }

    /// Column `pos` as the fold left it, with the piece it still holds.
    pub fn column(&self, pos: ChunkPos) -> Option<(ColumnPayload, Option<PieceRange>)> {
        Some((
            self.world.column_content(pos)?,
            self.world.origin_of(Resident::Column(pos)),
        ))
    }

    pub fn column_sections(&self, pos: ChunkPos) -> Vec<SectionPos> {
        self.world.column_sections(pos)
    }

    pub fn section(&self, pos: SectionPos) -> Option<(SectionContent, Option<PieceRange>)> {
        self.world.section_content(pos)
    }
}
