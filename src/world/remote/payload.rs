use std::sync::Arc;

use crate::chunk::{ChunkPos, SectionPos, SECTION_SIZE};
use crate::net::protocol::{
    ColumnPayload, LightPayload, SectionBytes, SectionPayload, SectionStatesPayload,
};
use crate::section::Section;
use crate::world::store::World;

use super::sorted_entries;

impl Section {
    /// Snapshot this section as its wire payload: `Arc` refcount bumps for the
    /// block/water/light buffers (no copies) plus the sparse state maps,
    /// encoded losslessly. Baked light rides along on EVERY transport — the
    /// ship gate (`section_light_final`) guarantees it is present unless the
    /// section never bakes (fully opaque); replica INGEST does no light work.
    pub(crate) fn to_payload(&self) -> SectionPayload {
        let mut cell_kv: Vec<crate::net::protocol::CellKvEntry> = self
            .cell_kv()
            .iter()
            .map(|(&cell, map)| {
                // BTreeMap iteration is key-sorted: deterministic on the wire.
                let entries = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                (cell, entries)
            })
            .collect();
        cell_kv.sort_unstable_by_key(|(cell, _)| *cell);

        SectionPayload {
            pos: SectionPos::new(self.cx, self.cy, self.cz),
            blocks: SectionBytes(self.blocks_arc()),
            metrics: self.stream_metrics(),
            water: self.water_arc().map(SectionBytes),
            skylight: self.skylight_arc().map(SectionBytes),
            blocklight: self.blocklight_arc().map(SectionBytes),
            states: SectionStatesPayload {
                doors: sorted_entries(self.doors(), |s| s.encode()),
                stairs: sorted_entries(self.stair_states(), |s| s.encode()),
                slabs: sorted_entries(self.slab_states(), |s| {
                    [s.encode_meta(), s.layers[0].0, s.layers[1].0]
                }),
                log_axes: sorted_entries(self.log_axes(), |a| a.to_u8()),
                torches: sorted_entries(self.torches(), |t| t.to_u8()),
                entity_facings: sorted_entries(self.entity_facings(), |f| f.to_u8()),
                model_facings: sorted_entries(self.model_facings(), |f| f.to_u8()),
                model_cells: sorted_entries(self.model_cells(), |&off| off),
                cell_kv,
            },
        }
    }
}

impl World {
    /// One column's client-relevant facts: biome skin, visible surface,
    /// direct-sky cover, and a per-cy [`SectionSummary`] for the whole world
    /// height range so replica physics can answer for absent sections. `None`
    /// for an unloaded column.
    pub(crate) fn column_payload(&self, pos: ChunkPos) -> Option<ColumnPayload> {
        let col = self.columns.get(&pos)?;
        let mut biomes = vec![0u8; SECTION_SIZE * SECTION_SIZE];
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                biomes[z * SECTION_SIZE + x] = col.biome_at(x, z);
            }
        }
        let summaries = Self::column_section_range()
            .map(|cy| {
                self.section_summary(SectionPos::new(pos.cx, cy, pos.cz))
                    .to_u8()
            })
            .collect();
        let (mesh_biomes, deep_band_lo) = self.column_gen.get(&pos).map_or_else(
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
                    crate::chunk::SECTION_MIN_CY,
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

    /// One loaded section's wire payload, or `None` when it isn't loaded.
    pub(crate) fn section_payload(&self, pos: SectionPos) -> Option<SectionPayload> {
        self.sections.get(&pos).map(|s| s.to_payload())
    }

    /// One section's CURRENT light cubes as a wire payload; `None` when the
    /// section is gone (an eviction race) or has never baked.
    pub(crate) fn light_payload(&self, pos: SectionPos) -> Option<LightPayload> {
        let s = self.sections.get(&pos)?;
        Some(LightPayload {
            pos,
            skylight: SectionBytes(s.skylight_arc()?),
            blocklight: s.blocklight_arc().map(SectionBytes),
        })
    }
}
