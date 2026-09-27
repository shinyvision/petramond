//! Landing a prepared restatement on a replica.
//!
//! A presentation prepares what an apply changes off the frame: decoded,
//! folded and compared against what is presented. What reaches the replica
//! is only what DIFFERS, as ready content, and landing it is a pointer swap
//! plus the ordinary install seam per key: neighbour meshes dirty exactly as
//! for a streamed install, and a key whose content did not change is never
//! touched, keeping its mesh, light bake and revision.

use std::sync::Arc;

use petramond_world::chunk::{ChunkPos, SectionPos};

use super::provenance::{Cached, PieceRange, Resident, SectionContent};
use crate::world::replication::ColumnPayload;
use crate::world::store::for_each_column_cy;
use crate::world::{ReplicaWorld, WorldData};

impl ReplicaWorld {
    /// Install a section whose content was prepared elsewhere, holding the
    /// piece at `origin`. The caller batches the returned position into
    /// [`finish_remote_install_batch`](Self::finish_remote_install_batch).
    pub fn install_prepared_section(
        &mut self,
        pos: SectionPos,
        content: SectionContent,
        origin: Option<PieceRange>,
    ) -> SectionPos {
        self.before_section_write(pos);
        self.put_section(pos, content.section, &content.draws);
        self.set_origin(Resident::Section(pos), origin);
        if let Some(range) = origin {
            self.uncache_piece(&range);
        }
        pos
    }

    /// Install a column whose content was prepared elsewhere.
    pub fn install_prepared_column(&mut self, payload: ColumnPayload, origin: Option<PieceRange>) {
        self.expect_origins([origin]);
        self.install_remote_column(payload);
        if let Some(range) = origin {
            self.uncache_piece(&range);
        }
    }

    /// Section `pos` as installed, with the piece it holds exactly.
    pub fn section_content(&self, pos: SectionPos) -> Option<(SectionContent, Option<PieceRange>)> {
        let section = Arc::clone(self.data.sections.get(&pos)?);
        Some((
            SectionContent {
                section,
                draws: self.section_block_draws(pos),
            },
            self.origin_of(Resident::Section(pos)),
        ))
    }

    /// Everything a column install wrote, read back raw: installing it again
    /// writes exactly this. `None` when no install wrote the column (it only
    /// stands under sections).
    pub fn column_content(&self, pos: ChunkPos) -> Option<ColumnPayload> {
        let halo = self.data.column_biome_halos.get(&pos)?;
        let col = self.data.columns.get(&pos)?;
        let side = petramond_world::chunk::SECTION_SIZE;
        let mut biomes = vec![0u8; side * side];
        for z in 0..side {
            for x in 0..side {
                biomes[z * side + x] = col.biome_at(x, z);
            }
        }
        let summaries = match self.data.column_summaries.get(&pos) {
            Some(s) => s.iter().map(|s| s.to_u8()).collect(),
            None => WorldData::column_section_range()
                .map(|cy| {
                    self.data
                        .unloaded_section_summary(SectionPos::new(pos.cx, cy, pos.cz))
                        .to_u8()
                })
                .collect(),
        };
        Some(ColumnPayload {
            pos,
            biomes: crate::world::replication::SectionBytes(Arc::from(biomes.into_boxed_slice())),
            mesh_biomes: crate::world::replication::SectionBytes(Arc::clone(halo)),
            surface_heightmap: col.surface_heightmap_slice().to_vec(),
            sky_cover: col.sky_cover_slice().to_vec(),
            summaries,
            deep_band_lo: self
                .data
                .column_deep_band_los
                .get(&pos)
                .copied()
                .unwrap_or(petramond_world::chunk::SECTION_MIN_CY),
        })
    }

    /// The installed sections of column `pos`.
    pub fn column_sections(&self, pos: ChunkPos) -> Vec<SectionPos> {
        let bits = self.data.section_column_cys.get(&pos).copied().unwrap_or(0);
        let mut out = Vec::with_capacity(bits.count_ones() as usize);
        for_each_column_cy(bits, |cy| out.push(SectionPos::new(pos.cx, cy, pos.cz)));
        out
    }

    /// Unload a presented section through the ordinary unload path; what it
    /// held from a piece stays in the cache.
    pub fn unload_presented_section(&mut self, pos: SectionPos) {
        if self.data.sections.contains_key(&pos) {
            self.uninstall_remote_section(pos);
        }
    }

    /// [`unload_presented_section`](Self::unload_presented_section) for a
    /// whole column and its sections.
    pub fn unload_presented_column(&mut self, pos: ChunkPos) {
        if !self.data.columns.contains_key(&pos) {
            return;
        }
        for sp in self.column_sections(pos) {
            self.before_section_write(sp);
        }
        self.uninstall_remote_column(pos);
    }

    /// `resident` holds exactly the piece at `origin`: its content was
    /// compared equal to it.
    pub fn set_presented_origin(&mut self, resident: Resident, origin: Option<PieceRange>) {
        // The range it held states this very content too: keep it for that
        // range, so passing it again costs nothing.
        if let Some(old) = self.origin_of(resident).filter(|&old| Some(old) != origin) {
            let content = match resident {
                Resident::Section(pos) => {
                    self.section_content(pos).map(|(c, _)| Cached::Section(c))
                }
                Resident::Column(pos) => self
                    .column_content(pos)
                    .map(|c| Cached::Column(Arc::new(c))),
            };
            if let Some(content) = content {
                self.cache_piece(old, content);
            }
        }
        self.set_origin(resident, origin);
    }

    /// Keep `content` for `range` unless the cache already holds it.
    pub fn remember_piece(&mut self, range: PieceRange, content: impl FnOnce() -> Cached) {
        if self.cached_piece(&range).is_none() {
            self.cache_piece(range, content());
        }
    }
}

/// A section payload decoded into exactly what its install holds; `None`
/// for a malformed payload, which an install drops too.
pub fn decode_section_payload(
    payload: crate::world::replication::SectionPayload,
) -> Option<SectionContent> {
    let decoded = super::ingest::RemoteSection::decode(payload)?;
    Some(SectionContent {
        section: Arc::new(decoded.section),
        draws: decoded.draws,
    })
}
