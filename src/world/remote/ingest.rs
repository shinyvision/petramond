use crate::world::cell_change::{CellChange, ChangeKind};
use crate::world::store::for_each_column_cy;
use crate::world::ReplicaWorld;
use crate::world::WorldData;
use std::collections::BTreeMap;
use std::sync::Arc;

use rustc_hash::FxHashSet;

use crate::world::replication::{BlockDelta, ColumnPayload, LightPayload, SectionPayload};
use crate::world::store::LoadTarget;
use petramond_world::block::Block;
use petramond_world::chunk::{ChunkPos, SectionPos, SECTION_SIZE, SECTION_VOLUME};
use petramond_world::section::{CellMap, Section, SectionSummary};

impl ReplicaWorld {
    pub fn install_remote_column(&mut self, payload: ColumnPayload) {
        let origin = self.take_record_origin();
        let expected_sections = WorldData::column_section_range().count();
        if payload.biomes.0.len() != SECTION_SIZE * SECTION_SIZE
            || payload.mesh_biomes.0.len() != 20 * 20
            || payload.surface_heightmap.len() != SECTION_SIZE * SECTION_SIZE
            || payload.sky_cover.len() != SECTION_SIZE * SECTION_SIZE
            || payload.summaries.len() != expected_sections
        {
            return;
        }
        let pos = payload.pos;
        self.before_column_write(pos);
        let col = self.data.ensure_column(pos);
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let i = z * SECTION_SIZE + x;
                if let Some(&b) = payload.biomes.0.get(i) {
                    col.set_biome(x, z, b);
                }
                if let Some(&h) = payload.surface_heightmap.get(i) {
                    col.set_surface_y(x, z, h);
                }
                if let Some(&h) = payload.sky_cover.get(i) {
                    col.set_sky_cover_y(x, z, h);
                }
            }
        }
        let summaries: Box<[SectionSummary]> = payload
            .summaries
            .iter()
            .map(|&b| SectionSummary::from_u8(b))
            .collect();
        self.data.column_summaries.insert(pos, summaries);
        self.data
            .column_biome_halos
            .insert(pos, payload.mesh_biomes.0);
        self.data
            .column_deep_band_los
            .insert(pos, payload.deep_band_lo);
        self.reclassify_column_sections(pos);
        self.data.bump_column_payload_revision(pos);
        self.set_origin(super::Resident::Column(pos), origin);
    }

    pub(super) fn reclassify_column_sections(&mut self, pos: ChunkPos) {
        for cy in WorldData::column_section_range() {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            if self.data.sections.contains_key(&sp) {
                self.classify_deep_on_install(sp);
            }
        }
    }

    #[cfg(test)]
    pub fn install_remote_section(&mut self, payload: SectionPayload) {
        if let Some(pos) = self.install_remote_section_deferred(payload) {
            self.finish_remote_install_batch(&[pos]);
        }
    }

    pub fn install_remote_section_deferred(
        &mut self,
        payload: SectionPayload,
    ) -> Option<SectionPos> {
        let origin = self.take_record_origin();
        let decoded = RemoteSection::decode(payload)?;
        self.before_section_write(decoded.pos);
        let RemoteSection {
            pos,
            section,
            draws,
        } = decoded;
        self.put_section(pos, Arc::new(section), &draws);
        self.set_origin(super::Resident::Section(pos), origin);
        Some(pos)
    }

    pub(super) fn put_section(
        &mut self,
        pos: SectionPos,
        section: Arc<Section>,
        draws: &[crate::world::replication::BlockDrawEntry],
    ) {
        self.data.ensure_column(pos.chunk_pos());
        self.data.sections.insert(pos, section);
        self.note_section_loaded(pos);
        self.data.bump_column_payload_revision(pos.chunk_pos());
        self.forget_block_draws_in_section(pos);
        for (cell, prims) in draws {
            let (lx, ly, lz) = petramond_world::chunk::section_local(*cell as usize);
            let at = petramond_math::math::IVec3::new(
                pos.cx * 16 + lx as i32,
                pos.cy * 16 + ly as i32,
                pos.cz * 16 + lz as i32,
            );
            self.apply_remote_block_draw(at, prims.clone());
        }
        self.refresh_block_entity_index(pos);
        self.refresh_presented_index(pos);
        self.classify_deep_on_install(pos);
    }

    pub fn finish_remote_install_batch(&mut self, installed: &[SectionPos]) {
        if installed.is_empty() {
            return;
        }
        let mut affected = FxHashSet::default();
        for pos in installed {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        affected.insert(SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz));
                    }
                }
            }
        }
        for pos in affected {
            self.queue_dirty_mesh(pos);
            self.defer_stream_mesh(pos);
        }
        self.side.terrain.vis_dirty = true;
    }

    pub fn install_remote_light(&mut self, payload: LightPayload) {
        if payload.skylight.0.len() != SECTION_VOLUME
            || payload
                .blocklight
                .as_ref()
                .is_some_and(|b| b.0.len() != SECTION_VOLUME)
        {
            return;
        }
        let pos = payload.pos;
        if !self.data.sections.contains_key(&pos) {
            return;
        }
        self.before_section_write(pos);
        let s = self.data.section_mut(pos).expect("presence checked above");
        let first_bake = !s.has_baked_light();
        let mask = if first_bake {
            crate::world::light::REGION_ALL
        } else {
            let sky = crate::world::light::cube_region_changes(
                s.skylight_arc().as_deref(),
                &payload.skylight.0,
                petramond_world::chunk::SKY_FULL,
            );
            let blk = match &payload.blocklight {
                Some(b) => crate::world::light::cube_region_changes(
                    s.blocklight_arc().as_deref(),
                    &b.0,
                    petramond_world::light::LightRgb::ZERO,
                ),
                None => match s.blocklight_arc() {
                    Some(old) => crate::world::light::cube_region_changes(
                        Some(&old),
                        &crate::world::light::ZERO_CUBE,
                        petramond_world::light::LightRgb::ZERO,
                    ),
                    None => 0,
                },
            };
            sky | blk
        };
        s.set_skylight(payload.skylight.0);
        match payload.blocklight {
            Some(b) => s.set_blocklight(b.0),
            None => s.clear_blocklight(),
        }
        if mask == 0 {
            return;
        }
        s.dirty = true;
        s.mesh_revision = s.mesh_revision.wrapping_add(1);
        self.data.bump_lighting_revision();
        self.side.terrain.dirty_meshes.push(pos);
        if !first_bake {
            self.requeue_meshes_sampling_changed_regions(pos, mask);
        }
    }

    pub fn apply_remote_delta(&mut self, delta: BlockDelta) {
        let Some((pos, lx, ly, lz)) = WorldData::split_world(delta.pos.x, delta.pos.y, delta.pos.z)
        else {
            return;
        };
        if !self.data.sections.contains_key(&pos) {
            return;
        }
        self.before_section_write(pos);
        self.before_column_write(pos.chunk_pos());
        let old = {
            let section = self.data.section_mut(pos).expect("presence checked above");
            let old = section.block(lx, ly, lz);
            section.set_block_raw(lx, ly, lz, delta.block_id);
            if let Some(meta) = delta.fluid {
                section.set_fluid(lx, ly, lz, Block::from_id(delta.block_id), meta);
            }
            if let Some(state) = delta.state {
                section.set_cell_state(lx, ly, lz, state);
            }
            for (key, value) in delta.cell_kv {
                section.cell_kv_set(lx, ly, lz, key, value);
            }
            section.mark_light_clean();
            old
        };
        self.apply_cell_changes(&[CellChange::new(delta.pos, old, ChangeKind::Remote)]);
    }

    pub fn apply_remote_cell_kv(&mut self, kv: crate::world::replication::CellKvDelta) {
        let Some((pos, lx, ly, lz)) = WorldData::split_world(kv.pos.x, kv.pos.y, kv.pos.z) else {
            return;
        };
        if !self.data.sections.contains_key(&pos) {
            return;
        }
        self.before_section_write(pos);
        let section = self.data.section_mut(pos).expect("presence checked above");
        let affects_mesh = petramond_world::block::kv_key_affects_mesh(&kv.key);
        match kv.value {
            Some(value) => section.cell_kv_set(lx, ly, lz, kv.key, value),
            None => {
                section.cell_kv_remove(lx, ly, lz, &kv.key);
            }
        }
        let block = Block::from_id(self.data.chunk_block(kv.pos.x, kv.pos.y, kv.pos.z));
        self.mark_custom_bake_edit(kv.pos.x, kv.pos.y, kv.pos.z, block);
        if affects_mesh && !block.is_custom_shape() {
            self.queue_dirty_meshes_sampling_cell(kv.pos.x, kv.pos.y, kv.pos.z);
        }
    }

    pub fn set_replica_view_center(&mut self, cx: i32, cy: i32, cz: i32) {
        let target = LoadTarget::new(cx, cy, cz, self.data.render_dist);
        if self.data.last_load_target != Some(target) {
            self.data.last_load_target = Some(target);
            self.side.terrain.vis_dirty = true;
        }
    }

    pub fn uninstall_remote_section(&mut self, pos: SectionPos) -> Option<Arc<Section>> {
        let evicted = self.data.sections.get(&pos).cloned();
        if evicted.is_some() {
            self.before_section_write(pos);
        }
        self.remove_section(pos);
        self.side.terrain.vis_dirty = true;
        evicted
    }

    pub fn uninstall_remote_column(&mut self, pos: ChunkPos) -> Vec<(SectionPos, Arc<Section>)> {
        let bits = self.data.section_column_cys.get(&pos).copied().unwrap_or(0);
        let mut evicted = Vec::with_capacity(bits.count_ones() as usize);
        for_each_column_cy(bits, |cy| {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            if let Some(s) = self.data.sections.get(&sp) {
                evicted.push((sp, Arc::clone(s)));
            }
        });
        for (sp, _) in &evicted {
            self.before_section_write(*sp);
        }
        self.before_column_write(pos);
        self.remove_column(pos);
        self.side.terrain.vis_dirty = true;
        evicted
    }

    pub fn install_cached_section(&mut self, pos: SectionPos, section: Arc<Section>) -> SectionPos {
        self.before_section_write(pos);
        self.put_section(pos, section, &[]);
        pos
    }
}

pub(super) struct RemoteSection {
    pub(super) pos: SectionPos,
    pub(super) section: Section,
    pub(super) draws: Vec<crate::world::replication::BlockDrawEntry>,
}

impl RemoteSection {
    pub(super) fn decode(payload: SectionPayload) -> Option<Self> {
        let pos = payload.pos;
        if !SectionPos::cy_in_range(pos.cy) || payload.blocks.0.len() != SECTION_VOLUME {
            return None;
        }
        if payload
            .fluid
            .as_ref()
            .is_some_and(|w| w.0.len() != SECTION_VOLUME)
        {
            return None;
        }
        if !payload.metrics.valid() {
            return None;
        }
        let SectionPayload {
            pos: _,
            blocks,
            metrics,
            fluid,
            skylight,
            blocklight,
            states:
                crate::world::replication::SectionStatesPayload {
                    cell_states,
                    cell_kv,
                    draws,
                },
        } = payload;
        let cell_kv: CellMap<BTreeMap<String, Vec<u8>>> = cell_kv
            .into_iter()
            .map(|(cell, entries)| (cell, entries.into_iter().collect()))
            .collect();
        let mut section = Section::from_replica(
            pos.cx,
            pos.cy,
            pos.cz,
            petramond_world::section::BlockCube::from_ids(&blocks.0),
            fluid.map(|w| w.0),
            CellMap::new(),
            CellMap::new(),
            cell_states.into_iter().collect(),
            cell_kv,
            metrics,
        );
        match skylight.filter(|l| l.0.len() == SECTION_VOLUME) {
            Some(sky) => {
                section.set_skylight(sky.0);
                if let Some(bl) = blocklight.filter(|l| l.0.len() == SECTION_VOLUME) {
                    section.set_blocklight(bl.0);
                }
            }
            None => section.mark_light_clean(),
        }
        Some(Self {
            pos,
            section,
            draws,
        })
    }
}
