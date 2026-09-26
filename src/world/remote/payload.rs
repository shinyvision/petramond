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

impl<S: WorldSide> World<S> {
    /// One loaded section's wire payload, or `None` when it isn't loaded —
    /// what the server ships, and what a replica's copy of it re-encodes to
    /// (the section cache compares the two).
    pub fn section_payload(&self, pos: SectionPos) -> Option<SectionPayload> {
        let mut payload = self.data.sections.get(&pos)?.to_payload();
        payload.states.draws = self.section_block_draws(pos);
        Some(payload)
    }
}

impl ServerWorld {
    /// One column's client-relevant facts: biome skin, visible surface,
    /// direct-sky cover, and a per-cy `SectionSummary` for the whole world
    /// height range so replica physics can answer for absent sections. `None`
    /// for an unloaded column.
    pub fn column_payload(&self, pos: ChunkPos) -> Option<ColumnPayload> {
        let col = self.data.columns.get(&pos)?;
        let mut biomes = vec![0u8; SECTION_SIZE * SECTION_SIZE];
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                biomes[z * SECTION_SIZE + x] = col.biome_at(x, z);
            }
        }
        let summaries = WorldData::column_section_range()
            .map(|cy| {
                self.data
                    .section_summary(SectionPos::new(pos.cx, cy, pos.cz))
                    .to_u8()
            })
            .collect();
        let (mesh_biomes, deep_band_lo) = self.side.gen.column_gen.get(&pos).map_or_else(
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
        );
        Some(ColumnPayload {
            pos,
            biomes: SectionBytes(Arc::from(biomes.into_boxed_slice())),
            mesh_biomes: SectionBytes(mesh_biomes),
            surface_heightmap: col.surface_heightmap_slice().to_vec(),
            sky_cover: col.sky_cover_slice().to_vec(),
            summaries,
            deep_band_lo,
        })
    }

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
