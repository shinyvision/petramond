use crate::world::WorldData;
use crate::world::{ServerWorld, World, WorldSide};
use std::sync::Arc;

use crate::world::replication::{
    ColumnPayload, LightPayload, SectionBlocks, SectionBytes, SectionLight, SectionPayload,
    SectionStatesPayload,
};
use petramond_world::chunk::{ChunkPos, SectionPos, SECTION_SIZE};
use petramond_world::section::Section;

pub(crate) trait SectionPayloadExt {
    fn to_payload(&self) -> SectionPayload;
}

impl SectionPayloadExt for Section {
    fn to_payload(&self) -> SectionPayload {
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
            blocks: SectionBlocks({
                let mut ids = vec![0u16; petramond_world::chunk::SECTION_VOLUME];
                self.blocks().copy_ids(&mut ids);
                ids.into()
            }),
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
                draws: Vec::new(),
            },
        }
    }
}

pub(crate) fn detached_section_payload(
    section: &Section,
    draws: Vec<crate::world::replication::BlockDrawEntry>,
) -> SectionPayload {
    let mut payload = section.to_payload();
    payload.states.draws = draws;
    payload
}

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
    pub fn section_payload(&self, pos: SectionPos) -> Option<SectionPayload> {
        let mut payload = self.data.sections.get(&pos)?.to_payload();
        payload.states.draws = self.section_block_draws(pos);
        Some(payload)
    }

    pub fn column_payload(&self, pos: ChunkPos) -> Option<ColumnPayload> {
        self.column_payload_with(pos, |sp| self.data.section_summary(sp))
    }

    pub fn column_payload_with(
        &self,
        pos: ChunkPos,
        mut loaded_summary: impl FnMut(SectionPos) -> petramond_world::section::SectionSummary,
    ) -> Option<ColumnPayload> {
        let col = self.data.columns.get(&pos)?;
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
    pub fn light_payload(&self, pos: SectionPos) -> Option<LightPayload> {
        let s = self.data.sections.get(&pos)?;
        Some(LightPayload {
            pos,
            skylight: SectionBytes(s.skylight_arc()?),
            blocklight: s.blocklight_arc().map(SectionLight),
        })
    }
}
