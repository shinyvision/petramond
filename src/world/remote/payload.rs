use crate::world::WorldData;
use crate::world::{ServerWorld, World, WorldSide};
use std::sync::Arc;

use crate::world::replication::{
    ColumnPayload, LightPayload, SectionBlocks, SectionBytes, SectionLight, SectionPayload,
    SectionStatesPayload,
};
use petramond_world::chunk::{ChunkPos, SectionPos, SECTION_SIZE};
use petramond_world::section::Section;

/// Wire-payload building over [`Section`] — transport encoding owned by the
/// replication layer (the section type itself lives in the world crate).
pub(crate) trait SectionPayloadExt {
    fn to_payload(&self) -> SectionPayload;
}

impl SectionPayloadExt for Section {
    /// Snapshot this section as its wire payload: `Arc` refcount bumps for the
    /// block/fluid/light buffers (no copies) plus the sparse state maps,
    /// encoded losslessly. Baked light rides along on EVERY transport — the
    /// ship gate (`section_light_final`) guarantees it is present unless the
    /// section never bakes (fully opaque); replica INGEST does no light work.
    fn to_payload(&self) -> SectionPayload {
        // Both levels are ordered maps (cells ascending, then keys), so
        // identical state encodes identically on the wire.
        let cell_kv: Vec<crate::world::replication::CellKvEntry> = self
            .cell_kv()
            .iter()
            .map(|(&cell, map)| {
                let entries = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                (cell, entries)
            })
            .collect();

        SectionPayload {
            pos: SectionPos::new(self.cx, self.cy, self.cz),
            blocks: SectionBlocks(self.blocks_iter().collect()),
            metrics: self.stream_metrics(),
            fluid: self.fluid_arc().map(SectionBytes),
            skylight: self.skylight_arc().map(SectionBytes),
            blocklight: self.blocklight_arc().map(SectionLight),
            states: SectionStatesPayload {
                cell_states: self
                    .cell_states()
                    .iter()
                    .map(|(&cell, &s)| (cell, s))
                    .collect(),
                cell_kv,
                // Filled by `World::section_payload`: draw sets are world-level
                // records (keyed at a machine's ANCHOR, which need not be in
                // this section's own cells), so a section cannot see its own.
                draws: Vec::new(),
            },
        }
    }
}

/// A section's wire payload built from a detached `Arc` — for a consumer that
/// encodes off the world thread (a state capture's pieces). Draw sets
/// are world-level records, so the caller collects them with
/// [`World::section_block_draws`] while it still holds the world.
pub(crate) fn detached_section_payload(
    section: &Section,
    draws: Vec<crate::world::replication::BlockDrawEntry>,
) -> SectionPayload {
    let mut payload = section.to_payload();
    payload.states.draws = draws;
    payload
}

/// A replica column's own state as a payload, built from detached handles
/// (a capture's snapshot) off the world thread: the summaries as the column
/// STORES them (the wire's, whatever sections are loaded), the halo and the
/// band floor its own payload carried.
pub(crate) fn detached_column_payload(
    pos: ChunkPos,
    col: &petramond_world::column::Column,
    summaries: Option<&[petramond_world::section::SectionSummary]>,
    halo: Option<Arc<[u8]>>,
    deep_band_lo: Option<i32>,
) -> ColumnPayload {
    let summaries = match summaries {
        Some(s) => s.iter().map(|s| s.to_u8()).collect(),
        None => WorldData::column_section_range()
            .map(|_| petramond_world::section::SectionSummary::Unknown.to_u8())
            .collect(),
    };
    ColumnPayload {
        pos,
        biomes: column_biomes(col),
        mesh_biomes: SectionBytes(halo.unwrap_or_else(|| Arc::from(Vec::new().into_boxed_slice()))),
        surface_heightmap: col.surface_heightmap_slice().to_vec(),
        sky_cover: col.sky_cover_slice().to_vec(),
        summaries,
        deep_band_lo: deep_band_lo.unwrap_or(petramond_world::chunk::SECTION_MIN_CY),
    }
}

/// A column's own biome skin, one byte per cell.
fn column_biomes(col: &petramond_world::column::Column) -> SectionBytes {
    let mut biomes = vec![0u8; SECTION_SIZE * SECTION_SIZE];
    for z in 0..SECTION_SIZE {
        for x in 0..SECTION_SIZE {
            biomes[z * SECTION_SIZE + x] = col.biome_at(x, z);
        }
    }
    SectionBytes(Arc::from(biomes.into_boxed_slice()))
}

impl<S: WorldSide> World<S> {
    /// One loaded section's wire payload, or `None` when it isn't loaded —
    /// what the server ships, and what a replica's copy of it re-encodes to
    /// (the section cache compares the two).
    pub fn section_payload(&self, pos: SectionPos) -> Option<SectionPayload> {
        let mut payload = self.data.sections.get(&pos)?.to_payload();
        payload.states.draws = self.section_block_draws(pos);
        Some(payload)
    }

    /// One column's client-relevant facts: biome skin, visible surface,
    /// direct-sky cover, and a per-cy `SectionSummary` for the whole world
    /// height range so replica physics can answer for absent sections. `None`
    /// for an unloaded column. What the server ships, and what a replica's
    /// copy re-encodes to (a state capture's Column piece).
    pub fn column_payload(&self, pos: ChunkPos) -> Option<ColumnPayload> {
        self.column_payload_with(pos, |sp| self.data.section_summary(sp))
    }

    /// [`column_payload`](Self::column_payload), with each LOADED section's
    /// summary answered by `loaded_summary` — a caller already holding those
    /// sections spares the lookups.
    pub fn column_payload_with(
        &self,
        pos: ChunkPos,
        mut loaded_summary: impl FnMut(SectionPos) -> petramond_world::section::SectionSummary,
    ) -> Option<ColumnPayload> {
        let col = self.data.columns.get(&pos)?;
        // Only the loaded sections are looked up; the loaded-section index
        // says which they are.
        let loaded = self.data.section_column_cys.get(&pos).copied().unwrap_or(0);
        let summaries = WorldData::column_section_range()
            .enumerate()
            .map(|(i, cy)| {
                let sp = SectionPos::new(pos.cx, cy, pos.cz);
                if loaded & (1 << i) != 0 {
                    loaded_summary(sp)
                } else {
                    self.data.unloaded_section_summary(sp)
                }
                .to_u8()
            })
            .collect();
        // A replica holds no generation, but it kept the halo and band floor
        // its own ColumnPayload carried — re-encoding a replica column must
        // hand those back, not a guess.
        let replica_facts = self.data.column_biome_halos.get(&pos).map(|halo| {
            (
                Arc::clone(halo),
                self.data
                    .column_deep_band_los
                    .get(&pos)
                    .copied()
                    .unwrap_or(petramond_world::chunk::SECTION_MIN_CY),
            )
        });
        let (mesh_biomes, deep_band_lo) = match replica_facts {
            Some(facts) => facts,
            None => self.column_gen_facts(pos, col),
        };
        Some(ColumnPayload {
            pos,
            biomes: column_biomes(col),
            mesh_biomes: SectionBytes(mesh_biomes),
            surface_heightmap: col.surface_heightmap_slice().to_vec(),
            sky_cover: col.sky_cover_slice().to_vec(),
            summaries,
            deep_band_lo,
        })
    }

    /// The generation-side halo and band floor, or a clamped halo when the
    /// column never generated here.
    fn column_gen_facts(
        &self,
        pos: ChunkPos,
        col: &petramond_world::column::Column,
    ) -> (Arc<[u8]>, i32) {
        let gen = self.side.server().and_then(|s| s.gen.column_gen.get(&pos));
        gen.map_or_else(
            || {
                let mut halo = vec![0u8; 20 * 20];
                for z in 0..20 {
                    for x in 0..20 {
                        halo[z * 20 + x] =
                            col.biome_at(x.saturating_sub(2).min(15), z.saturating_sub(2).min(15));
                    }
                }
                (
                    Arc::from(halo.into_boxed_slice()),
                    petramond_world::chunk::SECTION_MIN_CY,
                )
            },
            |gen| {
                (
                    gen.mesh_biome(),
                    *Self::surface_window_for_column(gen, 0).start(),
                )
            },
        )
    }
}

impl ServerWorld {
    /// One section's CURRENT light cubes as a wire payload; `None` when the
    /// section is gone (an eviction race) or has never baked.
    pub fn light_payload(&self, pos: SectionPos) -> Option<LightPayload> {
        let s = self.data.sections.get(&pos)?;
        Some(LightPayload {
            pos,
            skylight: SectionBytes(s.skylight_arc()?),
            blocklight: s.blocklight_arc().map(SectionLight),
        })
    }
}
